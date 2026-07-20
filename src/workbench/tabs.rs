use ratatui::layout::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShellTabId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellTabs {
    tabs: Vec<ShellTabId>,
    active: ShellTabId,
    next: u64,
}

impl Default for ShellTabs {
    fn default() -> Self {
        Self {
            tabs: vec![ShellTabId(1)],
            active: ShellTabId(1),
            next: 2,
        }
    }
}

impl ShellTabs {
    pub fn ids(&self) -> impl Iterator<Item = ShellTabId> + '_ {
        self.tabs.iter().copied()
    }
    pub fn active(&self) -> ShellTabId {
        self.active
    }
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn new_tab(&mut self) -> ShellTabId {
        let id = ShellTabId(self.next);
        self.next = self.next.checked_add(1).expect("shell tab id exhausted");
        self.tabs.push(id);
        self.active = id;
        id
    }

    pub fn select(&mut self, id: ShellTabId) -> bool {
        if self.tabs.contains(&id) {
            self.active = id;
            true
        } else {
            false
        }
    }

    /// Removes a tab and returns `(removed, replacement)`. A replacement is
    /// allocated immediately when the last tab closes, so identities never
    /// get reused (including shell process exits).
    pub fn close(&mut self, id: ShellTabId) -> Option<(ShellTabId, Option<ShellTabId>)> {
        let index = self.tabs.iter().position(|candidate| *candidate == id)?;
        let was_active = self.active == id;
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            let replacement = ShellTabId(self.next);
            self.next = self.next.checked_add(1).expect("shell tab id exhausted");
            self.tabs.push(replacement);
            self.active = replacement;
            return Some((id, Some(replacement)));
        }
        if was_active {
            // The right neighbor moves into the removed index; if there was no
            // right neighbor, use the previous (now last) tab.
            self.active = self.tabs[index.min(self.tabs.len() - 1)];
        }
        Some((id, None))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellTabTarget {
    Body(ShellTabId),
    Close(ShellTabId),
    Plus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellTabGeometry {
    pub id: ShellTabId,
    /// One-based position in the current ordered tab list. Unlike `id`, this
    /// is presentation-only and closes gaps after tabs are removed.
    pub display_number: usize,
    pub x: u16,
    pub width: u16,
    pub close_x: u16,
}

pub const SHELL_TAB_ROW_OFFSET: u16 = 1;
const TAB_WIDTH: u16 = 7;

/// Returns a contiguous visible window. The active tab is always retained and
/// the final inner cell is permanently reserved for the plus button.
pub fn shell_tab_geometry(
    area: Rect,
    tabs: &ShellTabs,
) -> (Vec<ShellTabGeometry>, Option<(u16, u16)>) {
    if area.width <= 2 || area.height <= 2 {
        return (Vec::new(), None);
    }
    let inner = area.width - 2;
    let plus_x = area.x + area.width - 2;
    let available = inner.saturating_sub(1);
    let capacity = usize::from(available / TAB_WIDTH).max(1).min(tabs.len());
    let active = tabs
        .tabs
        .iter()
        .position(|id| *id == tabs.active)
        .unwrap_or(0);
    let mut start = active.saturating_sub(capacity / 2);
    start = start.min(tabs.len().saturating_sub(capacity));
    let mut result = Vec::with_capacity(capacity);
    let mut x = area.x + 1;
    for (offset, id) in tabs.tabs[start..start + capacity]
        .iter()
        .copied()
        .enumerate()
    {
        let width = TAB_WIDTH.min(plus_x.saturating_sub(x));
        if width == 0 {
            break;
        }
        result.push(ShellTabGeometry {
            id,
            display_number: start + offset + 1,
            x,
            width,
            close_x: x + width - 1,
        });
        x += width;
    }
    (result, Some((plus_x, area.y + SHELL_TAB_ROW_OFFSET)))
}

pub fn shell_tab_hit_test(
    area: Rect,
    tabs: &ShellTabs,
    column: u16,
    row: u16,
) -> Option<ShellTabTarget> {
    let (geometry, plus) = shell_tab_geometry(area, tabs);
    if plus == Some((column, row)) {
        return Some(ShellTabTarget::Plus);
    }
    geometry.into_iter().find_map(|tab| {
        (row == area.y + SHELL_TAB_ROW_OFFSET && column >= tab.x && column < tab.x + tab.width)
            .then_some(if column == tab.close_x {
                ShellTabTarget::Close(tab.id)
            } else {
                ShellTabTarget::Body(tab.id)
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_active_prefers_right_then_previous_and_inactive_preserves_active() {
        let three_tabs = || {
            let mut tabs = ShellTabs::default();
            let two = tabs.new_tab();
            let three = tabs.new_tab();
            (tabs, two, three)
        };

        let (mut first, two, _) = three_tabs();
        first.select(ShellTabId(1));
        first.close(ShellTabId(1));
        assert_eq!(first.active(), two);

        let (mut middle, two, three) = three_tabs();
        middle.select(two);
        middle.close(two);
        assert_eq!(middle.active(), three);

        let (mut last, two, three) = three_tabs();
        last.close(three);
        assert_eq!(last.active(), two);

        let (mut inactive, _, three) = three_tabs();
        inactive.close(ShellTabId(1));
        assert_eq!(inactive.active(), three);
    }

    #[test]
    fn closing_last_allocates_a_new_monotonic_identity() {
        let mut tabs = ShellTabs::default();
        let (_, replacement) = tabs.close(ShellTabId(1)).unwrap();
        assert_eq!(replacement, Some(ShellTabId(2)));
        assert_eq!(tabs.new_tab(), ShellTabId(3));
    }

    #[test]
    fn display_numbers_follow_current_order_not_historical_ids() {
        let mut tabs = ShellTabs::default();
        let two = tabs.new_tab();
        let three = tabs.new_tab();
        tabs.close(two);
        let four = tabs.new_tab();

        let (geometry, _) = shell_tab_geometry(Rect::new(0, 0, 40, 10), &tabs);
        assert_eq!(
            geometry
                .iter()
                .map(|tab| (tab.id, tab.display_number))
                .collect::<Vec<_>>(),
            vec![(ShellTabId(1), 1), (three, 2), (four, 3)]
        );
    }

    #[test]
    fn one_hundred_last_tab_replacements_never_reuse_identity() {
        let mut tabs = ShellTabs::default();
        let mut previous = tabs.active();
        for _ in 0..100 {
            let (_, replacement) = tabs.close(previous).unwrap();
            let replacement = replacement.expect("closing the last tab creates a replacement");
            assert!(replacement > previous);
            assert_eq!(tabs.len(), 1);
            assert_eq!(tabs.active(), replacement);
            let (geometry, _) = shell_tab_geometry(Rect::new(0, 0, 20, 6), &tabs);
            assert_eq!(geometry[0].display_number, 1);
            previous = replacement;
        }
        assert_eq!(previous, ShellTabId(101));
        assert_eq!(tabs.new_tab(), ShellTabId(102));
    }

    #[test]
    fn geometry_distinguishes_body_close_plus_and_keeps_active_in_overflow() {
        let mut tabs = ShellTabs::default();
        for _ in 0..8 {
            tabs.new_tab();
        }
        let area = Rect::new(10, 4, 24, 10);
        let (geometry, plus) = shell_tab_geometry(area, &tabs);
        assert!(geometry.iter().any(|tab| tab.id == tabs.active()));
        let active = geometry.iter().find(|tab| tab.id == tabs.active()).unwrap();
        assert_eq!(
            shell_tab_hit_test(area, &tabs, active.x, 5),
            Some(ShellTabTarget::Body(active.id))
        );
        assert_eq!(
            shell_tab_hit_test(area, &tabs, active.close_x, 5),
            Some(ShellTabTarget::Close(active.id))
        );
        let (x, y) = plus.unwrap();
        assert_eq!(
            shell_tab_hit_test(area, &tabs, x, y),
            Some(ShellTabTarget::Plus)
        );
    }
}
