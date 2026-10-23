use keyring::Entry;

const SERVICE: &str = "rustcord";
const USERNAME: &str = "session-token";

pub fn load_token() -> Option<String> {
    let entry = Entry::new(SERVICE, USERNAME).ok()?;
    entry.get_password().ok()
}

pub fn save_token(token: &str) {
    let Ok(entry) = Entry::new(SERVICE, USERNAME) else { return };
    if let Err(e) = entry.set_password(token) {
        tracing::warn!("failed to save token to keychain: {e}");
    }
}

pub fn clear_token() {
    let Ok(entry) = Entry::new(SERVICE, USERNAME) else { return };
    let _ = entry.delete_credential();
}
