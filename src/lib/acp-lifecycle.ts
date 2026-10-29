/** Fixed call-site names only; never include prompts, paths or credentials. */
export type AcpDisconnectSource =
  | "frontend_disconnect"
  | "frontend_idle_sweep"
  | "frontend_target_changed"
  | "frontend_abandoned_connect"
  | "frontend_superseded_connect"
  | "frontend_replaced_connection"
  | "frontend_disconnect_action"
  | "frontend_config_reapply"
  | "frontend_user_reconnect"
  | "frontend_disconnect_all"

let generation = 0

export function nextAcpLifecycleGeneration(): number {
  generation += 1
  return generation
}
