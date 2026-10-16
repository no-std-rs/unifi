#![cfg(unix)]

#[path = "../cli/src/process.rs"]
mod process;

#[test]
fn bounded_process_duplex_exit_capacity_and_timeout() {
    let input = [b'x'; 131072];
    let mut output = [0; 131072];
    let (n, status) = process::run(
        "/usr/bin/cat",
        &[],
        "",
        &input,
        &mut output,
        5,
        false,
        || false,
    )
    .unwrap();
    assert_eq!(status, 0);
    assert_eq!(n, input.len());
    assert_eq!(output, input);
    assert_eq!(
        process::run("/usr/bin/false", &[], "", &[], &mut output, 5, true, || {
            false
        })
        .unwrap()
        .1,
        1
    );
    assert!(
        process::run(
            "/usr/bin/cat",
            &[],
            "",
            b"two",
            &mut [0; 1],
            5,
            true,
            || false
        )
        .is_err()
    );
    assert_eq!(
        process::run(
            "/usr/bin/sleep",
            &["5"],
            "",
            &[],
            &mut output,
            1,
            true,
            || false
        )
        .unwrap_err(),
        "process timed out"
    );
    assert_eq!(
        process::run(
            "/usr/bin/sleep",
            &["5"],
            "",
            &[],
            &mut output,
            5,
            true,
            || true
        )
        .unwrap_err(),
        "process interrupted"
    );
}
