use keyring::Entry;

const SERVICE: &str = "rustcord";
const USERNAME: &str = "session-token";

pub fn load_token() -> Result<Option<String>, keyring::Error> {
    let entry = Entry::new(SERVICE, USERNAME)?;
    match entry.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn save_token(token: &str) -> Result<(), keyring::Error> {
    let entry = Entry::new(SERVICE, USERNAME)?;
    entry.set_password(token)
}

pub fn clear_token() -> Result<(), keyring::Error> {
    let entry = Entry::new(SERVICE, USERNAME)?;
    match entry.delete_credential() {
        Err(keyring::Error::NoEntry) => Ok(()),
        result => result,
    }
}
