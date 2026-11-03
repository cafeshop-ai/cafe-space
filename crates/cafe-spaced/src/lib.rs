#[cfg(feature = "dev-build")]
pub const DAEMON_NAME: &str = "cafe-spaced-dev";
#[cfg(not(feature = "dev-build"))]
pub const DAEMON_NAME: &str = "cafe-spaced";

#[cfg(feature = "dev-build")]
pub const DATA_DIR_NAME: &str = "cafe-space-dev";
#[cfg(not(feature = "dev-build"))]
pub const DATA_DIR_NAME: &str = "cafe-space";

pub fn run() {
    println!("{DAEMON_NAME} skeleton; daemon runtime is not implemented yet");
}

#[cfg(test)]
mod tests {
    use super::{DAEMON_NAME, DATA_DIR_NAME};

    #[test]
    fn binary_and_data_directory_names_match_the_build_flavor() {
        #[cfg(feature = "dev-build")]
        assert_eq!(
            (DAEMON_NAME, DATA_DIR_NAME),
            ("cafe-spaced-dev", "cafe-space-dev")
        );
        #[cfg(not(feature = "dev-build"))]
        assert_eq!((DAEMON_NAME, DATA_DIR_NAME), ("cafe-spaced", "cafe-space"));
    }
}
