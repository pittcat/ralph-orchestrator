//! U1 契约测试:`ralph_core::path::workspace::path_within_workspace`
//! 必须先 canonicalize 再按「路径组件」判断包含关系,防止
//! `starts_with` 字符串前缀误判与 symlink 链逃逸。

use ralph_core::path::workspace::path_within_workspace;

#[test]
fn path_within_workspace_happy_subpath() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path().join("work");
    let inner = ws.join("dag").join("plan-a");
    std::fs::create_dir_all(&inner).expect("mkdir");

    assert!(path_within_workspace(&ws, &inner).expect("probe"));
    assert!(
        path_within_workspace(&ws, &ws).expect("probe"),
        "workspace itself counts as contained"
    );
}

#[test]
fn path_within_workspace_rejects_component_boundary_breach() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path().join("work");
    // 同级兄弟目录,名字以 workspace 名为字符串前缀。
    let sibling = tmp.path().join("workbench").join("dag");
    std::fs::create_dir_all(&ws).expect("mkdir ws");
    std::fs::create_dir_all(&sibling).expect("mkdir sibling");

    assert!(
        !path_within_workspace(&ws, &sibling).expect("probe"),
        "`workbench/dag` must not be treated as inside `work`"
    );
}

#[test]
fn path_within_workspace_rejects_parent_and_shorter_paths() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path().join("work").join("nested");
    std::fs::create_dir_all(&ws).expect("mkdir");

    assert!(
        !path_within_workspace(&ws, tmp.path()).expect("probe"),
        "an ancestor of the workspace is not contained in it"
    );
}

#[test]
fn path_within_workspace_rejects_symlink_chain_escape() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path().join("work");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&ws).expect("mkdir ws");
    std::fs::create_dir_all(&outside).expect("mkdir outside");
    let secret = outside.join("secret");
    std::fs::write(&secret, b"x").expect("write");

    let link = ws.join("legit");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&secret, &link).expect("symlink");
    #[cfg(not(unix))]
    return;

    assert!(
        !path_within_workspace(&ws, &link).expect("probe"),
        "symlink resolving outside the workspace must be rejected"
    );
}
