pub(super) fn is_lower_hex(value: &str, expected_len: usize) -> bool {
    value.len() == expected_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn is_git_object_hex(value: &str) -> bool {
    is_lower_hex(value, 40) || is_lower_hex(value, 64)
}
