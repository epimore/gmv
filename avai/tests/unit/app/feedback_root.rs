use super::*;

fn test_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "avai-feedback-root-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn rejects_direct_overlap_in_both_directions() {
    let root = test_root();
    let object = root.join("objects");
    let model = root.join("models");
    std::fs::create_dir_all(&model).unwrap();
    for feedback in [
        object.clone(),
        object.join("feedback"),
        root.clone(),
        model.clone(),
        model.join("feedback"),
    ] {
        assert!(validate_feedback_root(&feedback, &object, &model).is_err());
    }
    let child_object = root.join("feedback/objects");
    let child_model = root.join("feedback/models");
    assert!(validate_feedback_root(&root.join("feedback"), &child_object, &model).is_err());
    std::fs::create_dir_all(&child_model).unwrap();
    assert!(validate_feedback_root(&root.join("feedback"), &object, &child_model).is_err());
    let independent = root.join("independent");
    assert_eq!(
        validate_feedback_root(&independent, &object, &model).unwrap(),
        independent.canonicalize().unwrap()
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[cfg(unix)]
#[test]
fn rejects_parent_symlink_alias() {
    use std::os::unix::fs::symlink;

    let root = test_root();
    let object = root.join("objects");
    let model = root.join("models");
    std::fs::create_dir_all(&object).unwrap();
    std::fs::create_dir_all(&model).unwrap();
    let alias = root.join("object-alias");
    symlink(&object, &alias).unwrap();
    assert!(validate_feedback_root(&alias.join("feedback"), &object, &model).is_err());
    let model_alias = root.join("model-alias");
    symlink(&model, &model_alias).unwrap();
    assert!(validate_feedback_root(&model_alias.join("feedback"), &object, &model).is_err());
    let final_link = root.join("feedback-link");
    symlink(&object, &final_link).unwrap();
    assert!(validate_feedback_root(&final_link, &object, &model).is_err());
    std::fs::remove_dir_all(&root).unwrap();
}
