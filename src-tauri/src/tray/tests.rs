use super::*;

#[test]
fn build_identity_identifies_the_exact_app_build() {
    assert!(BUILD_IDENTITY.contains(env!("CARGO_PKG_VERSION")));
    assert!(BUILD_IDENTITY.contains(env!("GIT_HASH")));
    assert!(BUILD_IDENTITY.contains(env!("BUILD_DATE")));
}
