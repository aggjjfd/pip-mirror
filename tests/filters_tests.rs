use pip_mirror::downloader::FileInfo;
use pip_mirror::filters;
use pip_mirror::resolver::types::TargetEnv;

const ACCEPTED_WHEELS: &[&str] = &[
    // 复合 manylinux tag（PEP 600 OR 语义）
    "tornado-6.5.5-cp39-abi3-manylinux1_x86_64.manylinux_2_28_x86_64.manylinux_2_5_x86_64.whl",
    // 单一 manylinux 标准
    "foo-1.0-py3-none-manylinux1_x86_64.whl",
    "foo-1.0-py3-none-manylinux2010_x86_64.whl",
    "foo-1.0-py3-none-manylinux2014_x86_64.whl",
    "foo-1.0-py3-none-manylinux_2_5_x86_64.whl",
    "foo-1.0-py3-none-manylinux_2_12_x86_64.whl",
    "foo-1.0-py3-none-manylinux_2_17_x86_64.whl",
    "foo-1.0-py3-none-manylinux_2_24_x86_64.whl",
    "foo-1.0-py3-none-manylinux_2_28_x86_64.whl",
    "foo-1.0-py3-none-manylinux_2_39_x86_64.whl",
    "foo-1.0-py3-none-linux_x86_64.whl",
    // Windows
    "foo-1.0-py3-none-win_amd64.whl",
    "foo-1.0-py3-none-win32.whl",
    // 复合接受 tag（两个都是接受的 manylinux 标准）
    "foo-1.0-py3-none-manylinux_2_17_x86_64.manylinux2014_x86_64.whl",
    // PEP 600 无限版本号 fallback
    "foo-1.0-py3-none-manylinux_2_27_x86_64.whl",
    "foo-1.0-py3-none-manylinux_2_31_x86_64.whl",
    // 复合 tag 中一个不在白名单、一个 fallback 也能过
    "foo-1.0-py3-none-manylinux_2_27_x86_64.manylinux_2_28_x86_64.whl",
    // pure
    "foo-1.0-py3-none-any.whl",
];

const REJECTED_WHEELS: &[&str] = &[
    // musl
    "foo-1.0-py3-none-musllinux_1_2_x86_64.whl",
    // macOS
    "foo-1.0-py3-none-macosx_10_9_x86_64.whl",
    "foo-1.0-py3-none-macosx_10_9_universal2.whl",
    // ARM
    "foo-1.0-py3-none-manylinux_2_28_aarch64.whl",
    "foo-1.0-py3-none-macosx_11_0_arm64.whl",
    "foo-1.0-py3-none-linux_armv7l.whl",
    "foo-1.0-py3-none-win_arm64.whl",
    // 其他架构
    "foo-1.0-py3-none-manylinux_2_28_s390x.whl",
    "foo-1.0-py3-none-manylinux_2_28_ppc64le.whl",
    "foo-1.0-py3-none-manylinux_2_28_riscv64.whl",
    "foo-1.0-py3-none-wasm32.whl",
    // 复合 tag 含拒绝子串
    "foo-1.0-py3-none-musllinux_1_2_x86_64.manylinux_2_28_x86_64.whl",
    // i686 不在接受列表
    "foo-1.0-py3-none-manylinux1_i686.whl",
    "foo-1.0-py3-none-manylinux2014_i686.whl",
];

const INVALID_FILENAMES: &[&str] = &[
    "foo-1.0.tar.gz",
    "foo-1.0.zip",
    "not-a-wheel.txt",
    "short.whl",
    "a-b-c.whl",
];

const NORMALIZE_CASES: &[(&str, &str)] = &[
    ("SomePackage", "somepackage"),
    ("some.package", "some-package"),
    ("some_package", "some-package"),
    ("Some.Package_Name", "some-package-name"),
    // PEP 503: 连续 [-_.] 折叠成单个 -
    ("foo--bar", "foo-bar"),
    ("Foo._Bar", "foo-bar"),
    // PEP 508 extras 一律剥掉(simple/<pkg>/ 不带 extras)
    ("markitdown[pptx,docx,xls,xlsx,pdf]", "markitdown"),
    ("Markitdown[pptx]", "markitdown"),
    ("requests", "requests"),
];

#[test]
fn test_accepted_wheels() {
    for filename in ACCEPTED_WHEELS {
        assert!(
            filters::is_accepted_wheel(filename),
            "expected accepted: {filename}"
        );
    }
}

#[test]
fn test_rejected_wheels() {
    for filename in REJECTED_WHEELS {
        assert!(
            !filters::is_accepted_wheel(filename),
            "expected rejected: {filename}"
        );
    }
}

#[test]
fn test_invalid_filenames() {
    for filename in INVALID_FILENAMES {
        assert!(
            !filters::is_accepted_wheel(filename),
            "expected rejected (invalid): {filename}"
        );
    }
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
    assert!(filters::is_source_distribution("foo-1.0.tar.bz2"));
    assert!(filters::is_source_distribution("foo-1.0.tar.xz"));
    assert!(!filters::is_source_distribution("foo-1.0-py3-none-any.whl"));
}

#[test]
fn test_normalize_name() {
    for (raw, expected) in NORMALIZE_CASES {
        assert_eq!(
            filters::normalize_package_name(raw),
            *expected,
            "normalize({raw})"
        );
    }
}

fn linux_target(py: &str) -> TargetEnv {
    TargetEnv::test_env("linux", "x86_64", py)
}

fn file_info(filename: &str) -> FileInfo {
    FileInfo {
        filename: filename.to_string(),
        url: format!("https://example.invalid/{filename}"),
        sha256: None,
        size: None,
        package_name: "pyarrow".to_string(),
        version: "24.0.0".to_string(),
    }
}

#[test]
fn test_wheel_installability_respects_python_minor() {
    let py312 = linux_target("3.12");
    assert!(filters::wheel_is_installable_for_target(
        "pyarrow-24.0.0-cp312-cp312-manylinux_2_28_x86_64.whl",
        &py312,
        "2.39",
    ));
    assert!(!filters::wheel_is_installable_for_target(
        "pyarrow-24.0.0-cp313-cp313-manylinux_2_28_x86_64.whl",
        &py312,
        "2.39",
    ));
    assert!(!filters::wheel_is_installable_for_target(
        "pyarrow-24.0.0-cp314-cp314-manylinux_2_28_x86_64.whl",
        &py312,
        "2.39",
    ));
    assert!(filters::wheel_is_installable_for_target(
        "hf_xet-1.5.0-cp39-abi3-manylinux_2_28_x86_64.whl",
        &py312,
        "2.39",
    ));
    assert!(!filters::wheel_is_installable_for_target(
        "hf_xet-1.5.0-cp313-abi3-manylinux_2_28_x86_64.whl",
        &py312,
        "2.39",
    ));
}

#[test]
fn test_select_files_skips_future_python_wheels() {
    let files = vec![
        file_info("pyarrow-24.0.0-cp312-cp312-manylinux_2_28_x86_64.whl"),
        file_info("pyarrow-24.0.0-cp313-cp313-manylinux_2_28_x86_64.whl"),
        file_info("pyarrow-24.0.0-cp314-cp314-manylinux_2_28_x86_64.whl"),
    ];
    let targets = TargetEnv::all_resolution_targets();
    let selected =
        filters::select_files_for_version(&files, &targets, true, "2.39");
    let names: Vec<&str> =
        selected.iter().map(|fi| fi.filename.as_str()).collect();

    assert!(
        names.contains(&"pyarrow-24.0.0-cp312-cp312-manylinux_2_28_x86_64.whl")
    );
    assert!(
        !names
            .contains(&"pyarrow-24.0.0-cp313-cp313-manylinux_2_28_x86_64.whl")
    );
    assert!(
        !names
            .contains(&"pyarrow-24.0.0-cp314-cp314-manylinux_2_28_x86_64.whl")
    );
}

#[test]
fn test_select_files_fallback_sdist_requires_only_source_distribution() {
    let targets = vec![linux_target("3.12")];
    let files = vec![
        file_info("demo-1.0.0-cp312-cp312-win_amd64.whl"),
        file_info("demo-1.0.0.tar.gz"),
    ];
    let selected =
        filters::select_files_for_version(&files, &targets, true, "2.39");
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].filename, "demo-1.0.0.tar.gz");

    let selected_without_source =
        filters::select_files_for_version(&files, &targets, false, "2.39");
    assert!(selected_without_source.is_empty());
}
