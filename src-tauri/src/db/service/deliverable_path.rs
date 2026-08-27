/// Canonical path representation used when a deliverable is written or
/// compared. Existing rows are deliberately not rewritten in bulk: older
/// databases may contain both slash styles, and rewriting one row in place can
/// collide with its legacy twin. New rows use `storage_*`; all lookups use
/// `identity`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeliverablePathIdentity {
    pub storage_root: String,
    pub storage_path: String,
    pub identity: String,
}

fn collapse_components(value: &str) -> Vec<String> {
    let mut components = Vec::new();
    for component in value.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            if components.last().is_some_and(|previous| previous != "..") {
                components.pop();
            } else {
                // Preserve an unsafe/unresolvable parent rather than silently
                // aliasing it to a valid user file. Write-side validation can
                // reject it; identity comparison must never guess.
                components.push(component.to_string());
            }
            continue;
        }
        components.push(component.to_string());
    }
    components
}

fn normalized_root(raw: &str) -> (String, bool) {
    let slash = raw.replace('\\', "/");
    let bytes = slash.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    let unc = slash.starts_with("//");
    let absolute = drive || unc || slash.starts_with('/');
    let windows_semantics = drive || unc;

    if drive {
        let drive_letter = (bytes[0] as char).to_ascii_uppercase();
        let rest = slash[2..].trim_start_matches('/');
        let components = collapse_components(rest);
        let suffix = components.join("/");
        return (
            if suffix.is_empty() {
                format!("{drive_letter}:/")
            } else {
                format!("{drive_letter}:/{suffix}")
            },
            true,
        );
    }

    let components = collapse_components(slash.trim_start_matches('/'));
    let suffix = components.join("/");
    let root = if unc {
        format!("//{suffix}")
    } else if absolute {
        format!("/{suffix}")
    } else {
        suffix
    };
    (
        if root == "/" {
            root
        } else {
            root.trim_end_matches('/').to_string()
        },
        windows_semantics,
    )
}

pub(crate) fn deliverable_path_identity(root_path: &str, path: &str) -> DeliverablePathIdentity {
    let (storage_root, windows_semantics) = normalized_root(root_path);
    // Deliverable paths are relative. Retain an unresolved leading `..` so it
    // cannot compare equal to a safe path; safely resolvable interior parents
    // are collapsed (for example `exports/draft/../final.pdf`).
    let storage_path = collapse_components(&path.replace('\\', "/")).join("/");
    let (identity_root, identity_path) = if windows_semantics {
        (storage_root.to_lowercase(), storage_path.to_lowercase())
    } else {
        (storage_root.clone(), storage_path.clone())
    };
    DeliverablePathIdentity {
        identity: format!("{identity_root}::{identity_path}"),
        storage_root,
        storage_path,
    }
}

#[cfg(test)]
mod tests {
    use super::deliverable_path_identity;

    #[test]
    fn windows_slashes_drive_case_and_safe_components_share_identity() {
        let canonical = deliverable_path_identity("D:/codeg/work", "exports/final/report.docx");
        for (root, path) in [
            ("d:\\codeg\\work", "exports\\final\\report.docx"),
            ("D://codeg/./work/", "exports/draft/../final/report.docx"),
        ] {
            assert_eq!(
                deliverable_path_identity(root, path).identity,
                canonical.identity
            );
        }
        assert_eq!(canonical.storage_root, "D:/codeg/work");
        assert_eq!(canonical.storage_path, "exports/final/report.docx");
    }

    #[test]
    fn posix_paths_remain_case_sensitive() {
        assert_ne!(
            deliverable_path_identity("/srv/Work", "Result.txt").identity,
            deliverable_path_identity("/srv/work", "result.txt").identity
        );
    }

    #[test]
    fn unresolved_parent_is_not_silently_aliased() {
        assert_ne!(
            deliverable_path_identity("D:/codeg/work", "../secret.txt").identity,
            deliverable_path_identity("D:/codeg/work", "secret.txt").identity
        );
    }
}
