use std::{fs, process::Command};

#[test]
fn logs_export_even_with_corrupt_library_and_while_library_is_locked() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let secret = "SENSITIVE-library-title-alib1.bot-token";
    fs::write(
        temp.path().join("library.json"),
        format!(r#"{{"recent_searches":["{secret}"]}}"#),
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_anime-levisione"))
        .arg("--data-dir")
        .arg(temp.path())
        .output()?;
    assert!(!output.status.success(), "no interactive terminal");
    let logs = fs::read_to_string(temp.path().join("logs/diagnostics.jsonl"))?;
    assert!(logs.contains("session_started"));
    assert!(logs.contains("core_failure"));
    assert!(!logs.contains(secret));
    assert!(!logs.contains(&temp.path().display().to_string()));
    // Export never opens/parses the library, nor obtains its exclusive lock.
    let _locked = anime_levisione_core::store::Store::open(Some(temp.path().to_path_buf()))?;
    fs::write(
        temp.path().join("library.json"),
        "corrupt sensitive metadata",
    )?;
    fs::write(temp.path().join("config.toml"), "invalid config")?;
    let exported = Command::new(env!("CARGO_BIN_EXE_anime-levisione"))
        .arg("--data-dir")
        .arg(temp.path())
        .arg("logs")
        .output()?;
    assert!(exported.status.success());
    assert_eq!(String::from_utf8(exported.stdout)?, logs);
    assert!(exported.stderr.is_empty());
    Ok(())
}

#[test]
fn demo_never_creates_diagnostic_files() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let _ = Command::new(env!("CARGO_BIN_EXE_anime-levisione"))
        .arg("--data-dir")
        .arg(temp.path())
        .arg("--demo")
        .output()?;
    assert_eq!(fs::read_dir(temp.path())?.count(), 0);
    Ok(())
}
