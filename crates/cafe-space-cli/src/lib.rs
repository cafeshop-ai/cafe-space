#[cfg(feature = "dev-build")]
pub const CLI_NAME: &str = "cafe-space-dev";
#[cfg(not(feature = "dev-build"))]
pub const CLI_NAME: &str = "cafe-space";

pub fn run() {
    println!("{CLI_NAME} skeleton; CLI commands are not implemented yet");
}

#[cfg(feature = "dev-build")]
pub const DATA_DIR_NAME: &str = "cafe-space-dev";
#[cfg(not(feature = "dev-build"))]
pub const DATA_DIR_NAME: &str = "cafe-space";

#[cfg(test)]
mod tests {
    use super::{CLI_NAME, DATA_DIR_NAME};

    #[test]
    fn binary_name_matches_the_build_flavor() {
        #[cfg(feature = "dev-build")]
        assert_eq!(
            (CLI_NAME, DATA_DIR_NAME),
            ("cafe-space-dev", "cafe-space-dev")
        );
        #[cfg(not(feature = "dev-build"))]
        assert_eq!((CLI_NAME, DATA_DIR_NAME), ("cafe-space", "cafe-space"));
    }
}
