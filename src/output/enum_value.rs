pub(super) fn unsigned_enum_value(value: i64, width: u8) -> u64 {
    match width {
        1 => value as u8 as u64,
        2 => value as u16 as u64,
        4 => value as u32 as u64,
        8 => value as u64,
        _ => unreachable!("enum widths are validated before rendering"),
    }
}

pub(super) fn format_enum_member_value(value: i64, width: u8) -> String {
    format!("{:#X}", unsigned_enum_value(value, width))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_large_and_negative_enum_values() {
        assert_eq!(format_enum_member_value(0x80000000, 4), "0x80000000");
        assert_eq!(format_enum_member_value(-2, 1), "0xFE");
        assert_eq!(format_enum_member_value(-2, 2), "0xFFFE");
        assert_eq!(format_enum_member_value(-2, 4), "0xFFFFFFFE");
        assert_eq!(format_enum_member_value(-2, 8), "0xFFFFFFFFFFFFFFFE");
    }
}
