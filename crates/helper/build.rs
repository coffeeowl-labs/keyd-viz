//! Link libsystemd for the logind active-session authz check (`sd_uid_get_state`,
//! see `src/authz.rs`). libsystemd ships on every systemd distro, which is the
//! deployment target; on a non-systemd host the daemon still builds elsewhere but
//! the `--active-session` policy is moot (use `--uid` there).
fn main() {
    println!("cargo:rustc-link-lib=systemd");
}
