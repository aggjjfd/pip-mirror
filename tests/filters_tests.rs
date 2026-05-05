use pip_mirror::filters;

#[test]
fn test_is_accepted_wheel_manylinux_x86_64() {
    assert!(filters::is_accepted_wheel(
        "foo-1.0-py3-none-manylinux1_x86_64.whl"
    ));
    assert!(filters::is_accepted_wheel(
        "foo-1.0-py3-none-manylinux_2_28_x86_64.whl"
    ));
    assert!(filters::is_accepted_wheel(
        "foo-1.0-py3-none-linux_x86_64.whl"
    ));
}

#[test]
fn test_is_accepted_wheel_win() {
    assert!(filters::is_accepted_wheel("foo-1.0-py3-none-win_amd64.whl"));
    assert!(filters::is_accepted_wheel("foo-1.0-py3-none-win32.whl"));
}

#[test]
fn test_rejected_musl_and_macos() {
    assert!(!filters::is_accepted_wheel(
        "foo-1.0-py3-none-musllinux_1_2_x86_64.whl"
    ));
    assert!(!filters::is_accepted_wheel(
        "foo-1.0-py3-none-macosx_10_9_x86_64.whl"
    ));
}

#[test]
fn test_pure_python() {
    assert!(filters::is_pure_python_wheel("foo-1.0-py3-none-any.whl"));
    assert!(!filters::is_pure_python_wheel(
        "foo-1.0-py3-none-win_amd64.whl"
    ));
}

#[test]
fn test_source_distribution() {
    assert!(filters::is_source_distribution("foo-1.0.tar.gz"));
    assert!(filters::is_source_distribution("foo-1.0.zip"));
    assert!(!filters::is_source_distribution("foo-1.0-py3-none-any.whl"));
}

#[test]
fn test_normalize_name() {
    assert_eq!(
        filters::normalize_package_name("SomePackage"),
        "somepackage"
    );
    assert_eq!(
        filters::normalize_package_name("some.package"),
        "some-package"
    );
    assert_eq!(
        filters::normalize_package_name("some_package"),
        "some-package"
    );
}

#[test]
fn test_composite_tag() {
    let t =
        "tornado-6.5.5-cp39-abi3-manylinux1_x86_64.manylinux_2_28_x86_64.whl";
    assert!(filters::is_accepted_wheel(t));

    let bad = "foo-1.0-py3-none-musllinux_1_2_x86_64.manylinux_2_28_x86_64.whl";
    assert!(!filters::is_accepted_wheel(bad));
}
