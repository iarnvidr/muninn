//! Both CLI entry points must remain clients, never additional service owners.
use std::path::Path;
use std::process::{Command, Output};

fn cli(data_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_muninn-cli"))
        .env("MUNINN_DATA_DIR", data_dir)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn help_does_not_initialize_a_service_or_data_directory() {
    let root = tempfile::tempdir().unwrap();
    let data_dir = root.path().join("uninitialized");
    for args in [&[][..], &["help"][..], &["--help"][..]] {
        let result = cli(&data_dir, args);
        assert!(result.status.success());
        assert!(String::from_utf8_lossy(&result.stdout).contains("Muninn CLI"));
        assert!(!data_dir.exists());
    }
}

#[test]
fn live_commands_do_not_start_an_app_when_no_service_exists() {
    let root = tempfile::tempdir().unwrap();
    let data_dir = root.path().join("uninitialized");
    let result = cli(&data_dir, &["status"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("start Muninn first"));
    assert!(!data_dir.exists());
}

#[test]
fn offline_diagnostics_do_not_require_or_initialize_the_service() {
    let root = tempfile::tempdir().unwrap();
    let data_dir = root.path().join("data");
    std::fs::create_dir(&data_dir).unwrap();
    let output = root.path().join("diagnostics.zip");
    let result = cli(
        &data_dir,
        &["diagnostics", "export", output.to_str().unwrap()],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(serde_json::from_slice::<serde_json::Value>(&result.stdout).is_ok());
    assert!(output.is_file());
    assert_eq!(std::fs::read_dir(&data_dir).unwrap().count(), 0);
}
