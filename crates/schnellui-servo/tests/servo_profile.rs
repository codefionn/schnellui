#![cfg(feature = "servo-engine")]

use schnellui_servo::servo_engine::{ServoEngine, ServoEngineError};

#[test]
fn durable_profile_path_must_be_a_directory() {
    let directory = tempfile::tempdir().unwrap();
    let profile_path = directory.path().join("not-a-directory");
    std::fs::write(&profile_path, b"occupied").unwrap();

    let Err(ServoEngineError::ProfileDirectory { path, .. }) =
        ServoEngine::new_with_profile(320, 240, &profile_path)
    else {
        panic!("a file cannot be used as a durable Servo profile directory");
    };
    assert_eq!(path, profile_path);
}
