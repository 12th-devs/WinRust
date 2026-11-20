//! Bounded, owned snapshots: no foreign pointers survive into the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    pub enabled: bool,
    pub checked: bool,
    pub action: Action,
    pub children: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Command(u32),
    System(u32),
    Uri(&'static str),
    Keys(Vec<u16>),
    Volume(i32),
    VolumeSet(i32),
    BrightnessSet(isize, i32),
    Radio(&'static str),
    Media(u8),
    ControlCenter,
    Mute,
    Lock,
    Toggle(&'static str),
    Exit,
}

impl Item {
    pub fn new(label: impl Into<String>, action: Action) -> Self {
        Self {
            label: label.into(),
            enabled: true,
            checked: false,
            action,
            children: vec![],
        }
    }
    pub fn group(label: impl Into<String>, children: Vec<Item>) -> Self {
        Self {
            children,
            ..Self::new(label, Action::None)
        }
    }
    pub fn separator() -> Self {
        Self::new("", Action::None)
    }
}

pub fn menu_label(raw: &str) -> String {
    let mut chars = raw.chars().peekable();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c == '&' {
            if chars.peek() == Some(&'&') {
                out.push('&');
                chars.next();
            }
        } else if !c.is_control() || c == '\t' {
            out.push(c);
        }
    }
    out
}

/// Menus that do not fit remain reachable in a trailing overflow trigger.
pub fn visible_count(widths: &[i32], available: i32, overflow: i32) -> usize {
    if widths.iter().sum::<i32>() <= available {
        return widths.len();
    }
    let mut used = 0;
    widths
        .iter()
        .take_while(|&&w| {
            used += w;
            used + overflow <= available
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_literal_ampersands_and_shortcuts() {
        assert_eq!(menu_label("&Save && close\tCtrl+S"), "Save & close\tCtrl+S");
    }
    #[test]
    fn overflow_reserves_its_own_hit_target() {
        assert_eq!(visible_count(&[40, 50, 60], 150, 32), 3);
        assert_eq!(visible_count(&[40, 50, 60], 120, 32), 1);
        assert_eq!(visible_count(&[40], 20, 32), 0);
    }
}
