use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rand::Rng;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex};

use crate::chat_channel::error::ChatChannelError;
use crate::chat_channel::traits::ChatChannelBackend;
use crate::chat_channel::types::*;

const ILINK_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
const ILINK_CHANNEL_VERSION: &str = "1.0.2";
/// Maximum number of messages buffered while context_token is expired.
const MAX_PENDING_MESSAGES: usize = 50;
const MAX_SEEN_INBOUND_MESSAGES: usize = 1024;
const MAX_SEND_RETRIES: usize = 3;

/// Shared HTTP client for QR code auth requests (avoids re-creating TLS state).
fn qr_client() -> reqwest::Client {
    use std::sync::OnceLock;
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_default()
        })
        .clone()
}

// ── QR code auth types (public, used by commands) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeixinQrcodeInfo {
    pub qrcode_id: String,
    pub qrcode_img_content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeixinQrcodeStatus {
    pub status: String,
    /// bot_token and base_url are consumed by the _core command layer and
    /// stripped before the response reaches the frontend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

/// Frontend-safe subset of [`WeixinQrcodeStatus`] — no credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeixinQrcodeStatusPublic {
    pub status: String,
}

struct SendRequest<'a> {
    channel_id: i32,
    client: &'a reqwest::Client,
    base_url: &'a str,
    bot_token: &'a str,
    wechat_uin: &'a str,
    to_user_id: &'a str,
    context_token: &'a str,
    text: &'a str,
    reply_context: &'a Mutex<Option<WeixinReplyContext>>,
    pending_messages: &'a Mutex<Vec<String>>,
}

fn send_retry_delay(retry: usize) -> Duration {
    let seconds = 1u64 << retry.min(2);
    #[cfg(test)]
    return Duration::from_millis(seconds);
    #[cfg(not(test))]
    Duration::from_secs(seconds)
}

fn retryable_send_status(status: reqwest::StatusCode) -> bool {
    status.is_server_error() || matches!(status.as_u16(), 408 | 425 | 429)
}

fn weixin_message_key(message: &serde_json::Value) -> Option<String> {
    ["message_id", "msg_id", "client_id"]
        .into_iter()
        .find_map(|field| {
            let value = message.get(field)?;
            value
                .as_str()
                .map(|id| format!("{field}:{id}"))
                .or_else(|| value.as_i64().map(|id| format!("{field}:{id}")))
        })
}

// ── QR code auth functions (called before backend exists) ──

pub async fn weixin_get_qrcode() -> Result<WeixinQrcodeInfo, ChatChannelError> {
    let client = qr_client();
    let resp = client
        .get(format!("{ILINK_BASE_URL}/ilink/bot/get_bot_qrcode"))
        .query(&[("bot_type", "3")])
        .send()
        .await
        .map_err(|e| ChatChannelError::ConnectionFailed(format!("QR code request failed: {e}")))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ChatChannelError::ConnectionFailed(format!("QR code parse failed: {e}")))?;

    let qrcode_id = body
        .get("qrcode")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let raw_img = body
        .get("qrcode_img_content")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    if qrcode_id.is_empty() {
        return Err(ChatChannelError::ConnectionFailed(
            "Empty qrcode in response".into(),
        ));
    }

    // If the image content is a URL, try to fetch the actual image bytes.
    // If the URL points to an HTML SPA (which renders the QR code via JS),
    // generate the QR code ourselves — the SPA simply encodes the page URL.
    let qrcode_img_content = if raw_img.starts_with("http://") || raw_img.starts_with("https://") {
        match fetch_image_as_data_uri(&client, &raw_img).await {
            Ok(data_uri) => data_uri,
            Err(_) => {
                tracing::info!("[Weixin] URL is an SPA page, generating QR code from URL");
                generate_qrcode_data_uri(&raw_img)?
            }
        }
    } else {
        raw_img
    };

    Ok(WeixinQrcodeInfo {
        qrcode_id,
        qrcode_img_content,
    })
}

/// Fetch an image from a URL and return it as a `data:<mime>;base64,...` string.
///
/// Returns an error if the URL points to an HTML page (SPA) rather than a
/// raw image — the caller will generate a QR code from the URL instead.
async fn fetch_image_as_data_uri(
    client: &reqwest::Client,
    url: &str,
) -> Result<String, ChatChannelError> {
    let resp = client
        .get(url)
        .header(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        )
        .header(reqwest::header::REFERER, ILINK_BASE_URL)
        .send()
        .await
        .map_err(|e| ChatChannelError::ConnectionFailed(format!("Image fetch failed: {e}")))?;

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/png")
        .to_string();

    if content_type.contains("text/html") || content_type.contains("text/plain") {
        return Err(ChatChannelError::ConnectionFailed(
            "QR code URL is an SPA page".into(),
        ));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ChatChannelError::ConnectionFailed(format!("Image read failed: {e}")))?;

    if bytes.is_empty() {
        return Err(ChatChannelError::ConnectionFailed(
            "Empty image response".into(),
        ));
    }
    let b64 = B64.encode(&bytes);
    let mime = content_type.split(';').next().unwrap_or("image/png").trim();
    Ok(format!("data:{mime};base64,{b64}"))
}

/// Generate a QR code image encoding the given text and return as a PNG data URI.
///
/// The iLink QR page is a SPA that renders `window.location.href` as a QR code.
/// We replicate that logic server-side so the frontend can display it directly.
fn generate_qrcode_data_uri(content: &str) -> Result<String, ChatChannelError> {
    use image::{codecs::png::PngEncoder, ImageEncoder, Luma};
    use qrcode::QrCode;

    let code = QrCode::new(content.as_bytes()).map_err(|e| {
        ChatChannelError::ConnectionFailed(format!("QR code generation failed: {e}"))
    })?;

    let img = code
        .render::<Luma<u8>>()
        .quiet_zone(true)
        .min_dimensions(250, 250)
        .build();
    let (w, h) = (img.width(), img.height());

    let mut png_buf: Vec<u8> = Vec::new();
    PngEncoder::new(&mut png_buf)
        .write_image(img.as_raw(), w, h, image::ExtendedColorType::L8)
        .map_err(|e| ChatChannelError::ConnectionFailed(format!("PNG encoding failed: {e}")))?;

    let b64 = B64.encode(&png_buf);
    Ok(format!("data:image/png;base64,{b64}"))
}

pub async fn weixin_check_qrcode(qrcode: &str) -> Result<WeixinQrcodeStatus, ChatChannelError> {
    let client = qr_client();
    let resp = client
        .get(format!("{ILINK_BASE_URL}/ilink/bot/get_qrcode_status"))
        .query(&[("qrcode", qrcode)])
        .send()
        .await
        .map_err(|e| {
            ChatChannelError::ConnectionFailed(format!("QR status request failed: {e}"))
        })?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ChatChannelError::ConnectionFailed(format!("QR status parse failed: {e}")))?;

    let status = body
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("waiting")
        .to_string();

    let bot_token = body
        .get("bot_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let base_url = body
        .get("baseurl")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(WeixinQrcodeStatus {
        status,
        bot_token,
        base_url,
    })
}

// ── Backend implementation ──

struct WeixinReplyContext {
    to_user_id: String,
    context_token: String,
    expired: bool,
}

pub struct WeixinBackend {
    bot_token: String,
    base_url: String,
    client: reqwest::Client,
    status: Arc<Mutex<ChannelConnectionStatus>>,
    channel_id: i32,
    shutdown_tx: Arc<Mutex<Option<tokio::sync::watch::Sender<bool>>>>,
    reply_context: Arc<Mutex<Option<WeixinReplyContext>>>,
    /// Messages that failed due to expired context_token, resend on next refresh.
    pending_messages: Arc<Mutex<Vec<String>>>,
    /// Stable X-WECHAT-UIN value for this backend instance.
    wechat_uin: String,
}

impl WeixinBackend {
    pub fn new(channel_id: i32, bot_token: String, base_url: String) -> Self {
        let uin_raw = rand::thread_rng().gen::<u32>().to_string();
        let wechat_uin = B64.encode(uin_raw.as_bytes());

        Self {
            bot_token,
            base_url,
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(45))
                .build()
                .unwrap_or_default(),
            status: Arc::new(Mutex::new(ChannelConnectionStatus::Disconnected)),
            channel_id,
            shutdown_tx: Arc::new(Mutex::new(None)),
            reply_context: Arc::new(Mutex::new(None)),
            pending_messages: Arc::new(Mutex::new(Vec::new())),
            wechat_uin,
        }
    }

    fn build_headers(bot_token: &str, wechat_uin: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("Content-Type", HeaderValue::from_static("application/json"));
        headers.insert(
            "AuthorizationType",
            HeaderValue::from_static("ilink_bot_token"),
        );

        if let Ok(val) = HeaderValue::from_str(wechat_uin) {
            headers.insert("X-WECHAT-UIN", val);
        }

        let bearer = format!("Bearer {bot_token}");
        if let Ok(val) = HeaderValue::from_str(&bearer) {
            headers.insert("Authorization", val);
        }

        headers
    }

    /// Build the JSON body for the iLink sendmessage API.
    fn build_send_body(to_user_id: &str, context_token: &str, text: &str) -> serde_json::Value {
        serde_json::json!({
            "msg": {
                "from_user_id": "",
                "to_user_id": to_user_id,
                "client_id": format!("codeg-{}", uuid::Uuid::new_v4()),
                "message_type": 2,
                "message_state": 2,
                "context_token": context_token,
                "item_list": [{
                    "type": 1,
                    "text_item": { "text": text }
                }]
            },
            "base_info": { "channel_version": ILINK_CHANNEL_VERSION }
        })
    }

    /// Send a message via the iLink API and handle the response.
    /// Returns `Ok(true)` if sent, `Ok(false)` if buffered due to expired context.
    async fn do_send(req: SendRequest<'_>) -> Result<bool, ChatChannelError> {
        let body = Self::build_send_body(req.to_user_id, req.context_token, req.text);
        let url = format!("{}/ilink/bot/sendmessage", req.base_url);
        for attempt in 0..=MAX_SEND_RETRIES {
            // `body` (including client_id) is built once above and deliberately
            // reused across retries so iLink can deduplicate an uncertain send.
            let response = req
                .client
                .post(&url)
                .headers(Self::build_headers(req.bot_token, req.wechat_uin))
                .json(&body)
                .send()
                .await;
            let resp = match response {
                Ok(resp) => resp,
                Err(error) if attempt < MAX_SEND_RETRIES => {
                    let delay = send_retry_delay(attempt);
                    tracing::warn!(
                        stage = "weixin_send_retry",
                        channel_id = req.channel_id,
                        retry = attempt + 1,
                        delay_ms = delay.as_millis() as u64,
                        error = %error,
                        "[Weixin] transient send failure; retrying"
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                }
                Err(error) => {
                    tracing::error!(
                        stage = "weixin_send_failed_binding_preserved",
                        channel_id = req.channel_id,
                        attempts = attempt + 1,
                        error = %error,
                        "[Weixin] send failed; durable binding is unchanged"
                    );
                    return Err(ChatChannelError::SendFailed(error.to_string()));
                }
            };

            let status_code = resp.status();
            let resp_text = match resp.text().await {
                Ok(text) => text,
                Err(error) if attempt < MAX_SEND_RETRIES => {
                    let delay = send_retry_delay(attempt);
                    tracing::warn!(
                        stage = "weixin_send_retry",
                        channel_id = req.channel_id,
                        retry = attempt + 1,
                        delay_ms = delay.as_millis() as u64,
                        error = %error,
                        "[Weixin] response body read failed; retrying stable client_id"
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                }
                Err(error) => {
                    tracing::error!(
                        stage = "weixin_send_failed_binding_preserved",
                        channel_id = req.channel_id,
                        attempts = attempt + 1,
                        error = %error,
                        "[Weixin] response read failed; durable binding is unchanged"
                    );
                    return Err(ChatChannelError::SendFailed(error.to_string()));
                }
            };

            if !status_code.is_success() {
                let error = format!("HTTP {status_code}: {resp_text}");
                if retryable_send_status(status_code) && attempt < MAX_SEND_RETRIES {
                    let delay = send_retry_delay(attempt);
                    tracing::warn!(
                        stage = "weixin_send_retry",
                        channel_id = req.channel_id,
                        retry = attempt + 1,
                        delay_ms = delay.as_millis() as u64,
                        status = status_code.as_u16(),
                        "[Weixin] transient HTTP send failure; retrying"
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                }
                tracing::error!(
                    stage = "weixin_send_failed_binding_preserved",
                    channel_id = req.channel_id,
                    attempts = attempt + 1,
                    status = status_code.as_u16(),
                    "[Weixin] send failed; durable binding is unchanged"
                );
                return Err(ChatChannelError::SendFailed(error));
            }

            // Check for ret errors in response (e.g. -2 = context expired).
            if let Ok(resp_json) = serde_json::from_str::<serde_json::Value>(&resp_text) {
                if let Some(ret) = resp_json.get("ret").and_then(|v| v.as_i64()) {
                    if ret != 0 {
                        let errmsg = resp_json
                            .get("errmsg")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        tracing::info!("[Weixin] sendmessage ret={ret}, errmsg={errmsg}");

                        if ret == -2 {
                            if let Some(ref mut context) = *req.reply_context.lock().await {
                                context.expired = true;
                            }
                            let mut buffer = req.pending_messages.lock().await;
                            if buffer.len() < MAX_PENDING_MESSAGES {
                                buffer.push(req.text.to_string());
                            }
                            tracing::info!(
                                "[Weixin] context_token expired (ret=-2), buffered message"
                            );
                            return Ok(false);
                        }

                        tracing::error!(
                            stage = "weixin_send_failed_binding_preserved",
                            channel_id = req.channel_id,
                            ret,
                            "[Weixin] API rejected send; durable binding is unchanged"
                        );
                        return Err(ChatChannelError::SendFailed(format!("ret={ret}: {errmsg}")));
                    }
                }
            }
            return Ok(true);
        }
        unreachable!("send retry loop always returns")
    }

    async fn send_text(&self, text: &str) -> Result<SentMessageId, ChatChannelError> {
        // Extract context data under lock, then release
        let (to_user_id, context_token, expired) = {
            let guard = self.reply_context.lock().await;
            let ctx = guard.as_ref().ok_or_else(|| {
                ChatChannelError::SendFailed(
                    "No active WeChat conversation context. A user must message the bot first."
                        .into(),
                )
            })?;
            (
                ctx.to_user_id.clone(),
                ctx.context_token.clone(),
                ctx.expired,
            )
        };

        // If context is expired, buffer the message for resend on next refresh
        if expired {
            tracing::info!(
                "[Weixin] context expired, buffering message (len={})",
                text.len()
            );
            let mut buf = self.pending_messages.lock().await;
            if buf.len() < MAX_PENDING_MESSAGES {
                buf.push(text.to_string());
            } else {
                tracing::info!("[Weixin] pending buffer full, dropping message");
            }
            return Ok(SentMessageId(String::new()));
        }

        tracing::info!(
            channel_id = self.channel_id,
            recipient_key = %sender_log_key(&to_user_id),
            context_token_len = context_token.len(),
            text_len = text.len(),
            "[Weixin] sending message"
        );

        Self::do_send(SendRequest {
            channel_id: self.channel_id,
            client: &self.client,
            base_url: &self.base_url,
            bot_token: &self.bot_token,
            wechat_uin: &self.wechat_uin,
            to_user_id: &to_user_id,
            context_token: &context_token,
            text,
            reply_context: &self.reply_context,
            pending_messages: &self.pending_messages,
        })
        .await?;

        Ok(SentMessageId(String::new()))
    }
}

#[async_trait]
impl ChatChannelBackend for WeixinBackend {
    fn channel_type(&self) -> ChannelType {
        ChannelType::Weixin
    }

    async fn start(
        &self,
        command_tx: mpsc::Sender<IncomingCommand>,
    ) -> Result<(), ChatChannelError> {
        *self.status.lock().await = ChannelConnectionStatus::Connecting;

        tracing::info!(
            "[Weixin] start: base_url={}, token_len={}",
            self.base_url,
            self.bot_token.len()
        );

        // Verify auth by doing a quick getupdates with empty cursor
        let verify_body = serde_json::json!({
            "get_updates_buf": "",
            "base_info": { "channel_version": ILINK_CHANNEL_VERSION }
        });
        let url = format!("{}/ilink/bot/getupdates", self.base_url);
        tracing::info!("[Weixin] verify POST {url}");

        let resp = self
            .client
            .post(&url)
            .headers(Self::build_headers(&self.bot_token, &self.wechat_uin))
            .json(&verify_body)
            .send()
            .await
            .map_err(|e| ChatChannelError::ConnectionFailed(e.to_string()))?;

        let status_code = resp.status();
        let resp_text = resp
            .text()
            .await
            .map_err(|e| ChatChannelError::ConnectionFailed(e.to_string()))?;

        tracing::info!("[Weixin] verify response status={status_code}");
        tracing::debug!("[Weixin] verify response body={resp_text}");

        let verify_result: serde_json::Value = serde_json::from_str(&resp_text)
            .map_err(|e| ChatChannelError::ConnectionFailed(format!("JSON parse failed: {e}")))?;

        // iLink API auth failures come back as `{"errcode":-14,"errmsg":"session timeout"}`
        // (no `ret` field). Treat any non-zero errcode as authentication failure.
        if let Some(errcode) = verify_result.get("errcode").and_then(|v| v.as_i64()) {
            if errcode != 0 {
                let errmsg = verify_result
                    .get("errmsg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                return Err(ChatChannelError::AuthenticationFailed(format!(
                    "Weixin verification failed (errcode={errcode}): {errmsg}"
                )));
            }
        }

        let ret = verify_result.get("ret").and_then(|v| v.as_i64());

        // Check for known auth-failure codes
        if ret == Some(-14) {
            return Err(ChatChannelError::AuthenticationFailed(
                "Session expired (ret=-14), please re-authenticate".into(),
            ));
        }

        // The iLink API may omit the `ret` field or return non-zero on the first
        // call. Always extract the cursor if present — it's needed for polling.
        let initial_cursor = verify_result
            .get("get_updates_buf")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if let Some(r) = ret {
            if r != 0 {
                tracing::info!(
                    "[Weixin] verify returned ret={r}, but got cursor len={}",
                    initial_cursor.len()
                );
            }
        }

        *self.status.lock().await = ChannelConnectionStatus::Connected;

        // Start long-polling loop
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
        *self.shutdown_tx.lock().await = Some(shutdown_tx);

        let client = self.client.clone();
        let bot_token = self.bot_token.clone();
        let base_url = self.base_url.clone();
        let wechat_uin = self.wechat_uin.clone();
        let channel_id = self.channel_id;
        let status = self.status.clone();
        let reply_context = self.reply_context.clone();
        let pending_messages = self.pending_messages.clone();

        tokio::spawn(async move {
            let mut cursor = initial_cursor;
            let mut consecutive_errors: u32 = 0;
            let mut seen_message_keys = HashSet::new();
            let mut seen_message_order = VecDeque::new();

            loop {
                if *shutdown_rx.borrow() {
                    break;
                }

                let body = serde_json::json!({
                    "get_updates_buf": cursor,
                    "base_info": { "channel_version": ILINK_CHANNEL_VERSION }
                });

                let result = tokio::select! {
                    r = client
                        .post(format!("{base_url}/ilink/bot/getupdates"))
                        .headers(WeixinBackend::build_headers(&bot_token, &wechat_uin))
                        .json(&body)
                        .send() => r,
                    _ = shutdown_rx.changed() => break,
                };

                match result {
                    Ok(resp) => {
                        // Recover from error state after successful poll
                        consecutive_errors = 0;
                        {
                            let mut s = status.lock().await;
                            if *s == ChannelConnectionStatus::Error {
                                *s = ChannelConnectionStatus::Connected;
                            }
                        }

                        if let Ok(body) = resp.json::<serde_json::Value>().await {
                            let ret = body.get("ret").and_then(|v| v.as_i64());

                            // Always update cursor if present
                            if let Some(new_cursor) =
                                body.get("get_updates_buf").and_then(|v| v.as_str())
                            {
                                if !new_cursor.is_empty() {
                                    cursor = new_cursor.to_string();
                                }
                            }

                            // If ret is explicitly non-zero (not just missing), log it
                            if let Some(r) = ret {
                                if r != 0 {
                                    tracing::info!("[Weixin] getupdates ret={r}");
                                }
                                // Session expired — pause and wait for re-auth
                                if r == -14 {
                                    tracing::info!("[Weixin] session expired (ret=-14), pausing 30s");
                                    *status.lock().await = ChannelConnectionStatus::Error;
                                    tokio::time::sleep(Duration::from_secs(30)).await;
                                    continue;
                                }
                            }

                            // Process messages
                            if let Some(msgs) = body.get("msgs").and_then(|v| v.as_array()) {
                                if !msgs.is_empty() {
                                    tracing::info!("[Weixin] got {} message(s)", msgs.len());
                                }
                                for msg in msgs {
                                    // Only handle user messages (message_type=1),
                                    // skip bot echo (message_type=2)
                                    let msg_type = msg.get("message_type").and_then(|v| v.as_i64());
                                    if msg_type != Some(1) {
                                        continue;
                                    }

                                    if let Some(message_key) = weixin_message_key(msg) {
                                        if !seen_message_keys.insert(message_key.clone()) {
                                            tracing::info!(
                                                stage = "weixin_inbound_duplicate_ignored",
                                                channel_id,
                                                "[Weixin] duplicate inbound message ignored"
                                            );
                                            continue;
                                        }
                                        seen_message_order.push_back(message_key);
                                        if seen_message_order.len() > MAX_SEEN_INBOUND_MESSAGES {
                                            if let Some(expired) = seen_message_order.pop_front() {
                                                seen_message_keys.remove(&expired);
                                            }
                                        }
                                    }

                                    // Extract text from type=1 (text) or type=3 (voice-to-text)
                                    let text = msg
                                        .get("item_list")
                                        .and_then(|v| v.as_array())
                                        .and_then(|items| {
                                            items.iter().find_map(|item| {
                                                let t =
                                                    item.get("type").and_then(|v| v.as_i64())?;
                                                match t {
                                                    1 => item
                                                        .pointer("/text_item/text")
                                                        .and_then(|v| v.as_str()),
                                                    3 => item
                                                        .pointer("/voice_item/text")
                                                        .and_then(|v| v.as_str()),
                                                    _ => None,
                                                }
                                            })
                                        });

                                    let text = match text {
                                        Some(t) if !t.is_empty() => t,
                                        _ => {
                                            tracing::warn!("[Weixin] skipped non-text message");
                                            continue;
                                        }
                                    };

                                    let from_user_id = msg
                                        .get("from_user_id")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or_default();
                                    let context_token = msg
                                        .get("context_token")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or_default();

                                    // Store reply context for outbound messages
                                    // Single lock scope to avoid TOCTOU
                                    if !from_user_id.is_empty() && !context_token.is_empty() {
                                        let was_expired = {
                                            let mut guard = reply_context.lock().await;
                                            let was =
                                                guard.as_ref().map(|c| c.expired).unwrap_or(false);
                                            *guard = Some(WeixinReplyContext {
                                                to_user_id: from_user_id.to_string(),
                                                context_token: context_token.to_string(),
                                                expired: false,
                                            });
                                            was
                                        };

                                        // Resend buffered messages with fresh context
                                        if was_expired {
                                            let buffered: Vec<String> =
                                                pending_messages.lock().await.drain(..).collect();
                                            if !buffered.is_empty() {
                                                tracing::info!(
                                                    "[Weixin] context refreshed, resending {} buffered message(s)",
                                                    buffered.len()
                                                );
                                                for pending_text in &buffered {
                                                    let ok = WeixinBackend::do_send(SendRequest {
                                                        channel_id,
                                                        client: &client,
                                                        base_url: &base_url,
                                                        bot_token: &bot_token,
                                                        wechat_uin: &wechat_uin,
                                                        to_user_id: from_user_id,
                                                        context_token,
                                                        text: pending_text,
                                                        reply_context: &reply_context,
                                                        pending_messages: &pending_messages,
                                                    })
                                                    .await;
                                                    if let Err(e) = ok {
                                                        tracing::error!("[Weixin] resend error: {e}");
                                                        // Re-buffer remaining on hard error
                                                        let mut buf = pending_messages.lock().await;
                                                        if buf.len() < MAX_PENDING_MESSAGES {
                                                            buf.push(pending_text.clone());
                                                        }
                                                    }
                                                    // If do_send returned Ok(false), it
                                                    // already re-buffered internally.
                                                }
                                            }
                                        }
                                    }

                                    tracing::debug!(
                                        channel_id,
                                        sender_key = %sender_log_key(from_user_id),
                                        message_chars = text.chars().count(),
                                        "[Weixin] dispatching inbound message"
                                    );
                                    let send_result = command_tx
                                        .send(IncomingCommand {
                                            channel_id,
                                            sender_id: from_user_id.to_string(),
                                            command_text: text.to_string(),
                                            callback_data: None,
                                            target: ChannelMessageTarget::channel(channel_id),
                                            metadata: msg.clone(),
                                        })
                                        .await;
                                    if let Err(e) = send_result {
                                        tracing::error!("[Weixin] command_tx.send failed: {e}");
                                    }
                                }
                            }
                        } else {
                            tracing::error!("[Weixin] failed to parse response body");
                        }
                    }
                    Err(e) => {
                        consecutive_errors += 1;
                        tracing::error!("[Weixin] polling error ({consecutive_errors}): {e}");
                        *status.lock().await = ChannelConnectionStatus::Error;
                        // Exponential backoff: 5s, 10s, 20s, capped at 30s
                        let delay =
                            std::cmp::min(5 * 2u64.saturating_pow(consecutive_errors - 1), 30);
                        tokio::time::sleep(Duration::from_secs(delay)).await;
                    }
                }
            }
            *status.lock().await = ChannelConnectionStatus::Disconnected;
        });

        Ok(())
    }

    async fn stop(&self) -> Result<(), ChatChannelError> {
        if let Some(tx) = self.shutdown_tx.lock().await.take() {
            let _ = tx.send(true);
        }
        *self.status.lock().await = ChannelConnectionStatus::Disconnected;
        Ok(())
    }

    async fn status(&self) -> ChannelConnectionStatus {
        *self.status.lock().await
    }

    async fn send_message(&self, text: &str) -> Result<SentMessageId, ChatChannelError> {
        self.send_text(text).await
    }

    async fn send_rich_message(
        &self,
        message: &RichMessage,
    ) -> Result<SentMessageId, ChatChannelError> {
        let plain_text = message.to_plain_text();
        self.send_text(&plain_text).await
    }

    async fn test_connection(&self) -> Result<(), ChatChannelError> {
        let body = serde_json::json!({
            "get_updates_buf": "",
            "base_info": { "channel_version": ILINK_CHANNEL_VERSION }
        });

        let url = format!("{}/ilink/bot/getupdates", self.base_url);
        let resp = self
            .client
            .post(&url)
            .headers(Self::build_headers(&self.bot_token, &self.wechat_uin))
            .json(&body)
            .send()
            .await
            .map_err(|e| ChatChannelError::ConnectionFailed(e.to_string()))?;

        let status_code = resp.status();
        let resp_text = resp
            .text()
            .await
            .map_err(|e| ChatChannelError::ConnectionFailed(e.to_string()))?;

        tracing::info!("[Weixin] test_connection: status={status_code}");
        tracing::debug!("[Weixin] test_connection body={resp_text}");

        let resp_json: serde_json::Value = serde_json::from_str(&resp_text)
            .map_err(|e| ChatChannelError::ConnectionFailed(format!("Not valid JSON: {e}")))?;

        if !status_code.is_success() {
            return Err(ChatChannelError::AuthenticationFailed(format!(
                "HTTP {status_code}"
            )));
        }

        // Check for known auth-failure codes
        if let Some(ret) = resp_json.get("ret").and_then(|v| v.as_i64()) {
            if ret == -14 {
                return Err(ChatChannelError::AuthenticationFailed(
                    "Session expired (ret=-14)".into(),
                ));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::{Json, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone, Default)]
    struct SendServerState {
        attempts: Arc<AtomicUsize>,
        client_ids: Arc<Mutex<Vec<String>>>,
    }

    async fn flaky_send(
        State(state): State<SendServerState>,
        Json(body): Json<serde_json::Value>,
    ) -> (StatusCode, Json<serde_json::Value>) {
        let attempt = state.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        let client_id = body
            .pointer("/msg/client_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        state.client_ids.lock().await.push(client_id);
        if attempt < 3 {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "temporary" })),
            )
        } else {
            (StatusCode::OK, Json(serde_json::json!({ "ret": 0 })))
        }
    }

    #[tokio::test]
    async fn transient_send_failures_retry_with_one_stable_client_id() {
        let state = SendServerState::default();
        let app = Router::new()
            .route("/ilink/bot/sendmessage", post(flaky_send))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let reply_context = Mutex::new(Some(WeixinReplyContext {
            to_user_id: "wx-user".into(),
            context_token: "context".into(),
            expired: false,
        }));
        let pending_messages = Mutex::new(Vec::new());

        let sent = WeixinBackend::do_send(SendRequest {
            channel_id: 1,
            client: &reqwest::Client::new(),
            base_url: &format!("http://{address}"),
            bot_token: "test-token",
            wechat_uin: "test-uin",
            to_user_id: "wx-user",
            context_token: "context",
            text: "reply",
            reply_context: &reply_context,
            pending_messages: &pending_messages,
        })
        .await
        .unwrap();

        assert!(sent);
        assert_eq!(state.attempts.load(Ordering::SeqCst), 3);
        let client_ids = state.client_ids.lock().await;
        assert_eq!(client_ids.len(), 3);
        assert!(!client_ids[0].is_empty());
        assert!(client_ids.iter().all(|id| id == &client_ids[0]));
        server.abort();
    }

    #[test]
    fn inbound_dedup_uses_only_stable_provider_message_ids() {
        assert_eq!(
            weixin_message_key(&serde_json::json!({ "message_id": "m-1", "text": "a" })),
            Some("message_id:m-1".into())
        );
        assert_eq!(
            weixin_message_key(&serde_json::json!({ "msg_id": 42 })),
            Some("msg_id:42".into())
        );
        assert_eq!(
            weixin_message_key(&serde_json::json!({ "text": "same text" })),
            None,
            "identical user text without a provider id must remain a legitimate repeat"
        );
    }
}
