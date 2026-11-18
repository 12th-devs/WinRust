//! Reads the output of `reg query <key> /v <name>` (the installer and debloat both change and restore values through reg.exe).

/// The value line as (type, data), e.g. ("REG_DWORD", "0x1"). None = no value in the output.
/// reg.exe writes it as `    <name>    <type>    <data>`, four spaces apart; names and data may contain spaces themselves.
pub fn value(out: &str) -> Option<(&str, &str)> {
    out.lines().find_map(|l| {
        let mut f = l.strip_prefix("    ")?.splitn(3, "    ");
        let (_, ty) = (f.next()?, f.next()?);
        ty.starts_with("REG_").then(|| (ty, f.next().unwrap_or("").trim_end()))
    })
}

/// The value as a number when it is a REG_DWORD; None when missing or of another type.
pub fn dword(out: &str) -> Option<u32> {
    match value(out)? {
        ("REG_DWORD", d) => u32::from_str_radix(d.trim_start_matches("0x"), 16).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_reg_query_output() {
        let key = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\REG_X\r\n";
        assert_eq!(dword(&format!("{key}    AllowTelemetry    REG_DWORD    0x1a\r\n\r\n")), Some(26));
        assert_eq!(value(&format!("{key}    Debugger    REG_SZ    \"C:\\Program Files\\x.exe\" /e\r\n")), Some(("REG_SZ", "\"C:\\Program Files\\x.exe\" /e")));
        assert_eq!(value("    (Default)    REG_SZ    \r\n"), Some(("REG_SZ", "")));
        assert_eq!(dword("    V    REG_SZ    0x1\r\n"), None);
        assert_eq!(value("ERROR: The system was unable to find the specified registry key or value."), None);
    }
}
