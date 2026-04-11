use ignore::WalkBuilder;

pub(crate) const DEFAULT_WALK_POLICY: &str = "ripgrep-style: parent ignore + hidden + .ignore/.gitignore/git excludes + .zgignore; .zg/ always skipped";

pub(crate) fn apply_content_filters(builder: &mut WalkBuilder) -> &mut WalkBuilder {
    builder
        .standard_filters(true)
        .add_custom_ignore_filename(".zgignore")
}
