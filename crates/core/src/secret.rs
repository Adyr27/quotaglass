use std::fmt;
use zeroize::Zeroizing;

/// A token or key. Not `Serialize`, not `Display`, not `Clone`; zeroed on drop.
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_debug_redacts() {
        assert_eq!(
            format!("{:?}", Secret::new("tok-123".into())),
            "Secret(***)"
        );
    }
}
