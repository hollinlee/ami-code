use ratatui::layout::Rect;

/// A bordered terminal needs one content cell in each direction.
pub const MIN_TERMINAL_WIDTH: u16 = 4;
pub const MIN_TERMINAL_HEIGHT: u16 = 4;
/// Bordered Shell plus one tab-strip row and two parser-safe content rows.
pub const MIN_SHELL_PANE_HEIGHT: u16 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkbenchLayoutConfig {
    pub sidebar_width: u16,
    /// Explicit Nvim pane width after a manual divider drag. `None` keeps the
    /// default responsive 55/45 split.
    pub editor_width: Option<u16>,
    pub bottom_height: u16,
}

impl Default for WorkbenchLayoutConfig {
    fn default() -> Self {
        Self {
            sidebar_width: 24,
            editor_width: None,
            bottom_height: 12,
        }
    }
}

impl WorkbenchLayoutConfig {
    pub fn sidebar_shortage_width(self) -> u16 {
        self.sidebar_width
            .saturating_add(MIN_TERMINAL_WIDTH.saturating_mul(2))
    }

    pub fn bottom_shortage_height(self) -> u16 {
        self.bottom_height.saturating_add(MIN_TERMINAL_HEIGHT)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkbenchVisibility {
    pub sidebar: bool,
    pub bottom: bool,
}

impl Default for WorkbenchVisibility {
    fn default() -> Self {
        Self {
            sidebar: true,
            bottom: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkbenchLayout {
    pub sidebar: Rect,
    pub editor: Rect,
    pub agent: Rect,
    pub bottom: Rect,
    /// True when even two bordered terminals cannot be represented safely.
    pub compact: bool,
}

impl WorkbenchLayout {
    #[cfg(test)]
    pub fn calculate(area: Rect, config: WorkbenchLayoutConfig) -> Self {
        Self::calculate_visible(area, config, WorkbenchVisibility::default())
    }

    pub fn calculate_visible(
        area: Rect,
        config: WorkbenchLayoutConfig,
        visibility: WorkbenchVisibility,
    ) -> Self {
        let sidebar_width = if visibility.sidebar {
            config.sidebar_width.min(area.width)
        } else {
            0
        };
        let main_width = area.width.saturating_sub(sidebar_width);
        if main_width < MIN_TERMINAL_WIDTH.saturating_mul(2) || area.height < MIN_TERMINAL_HEIGHT {
            return Self {
                sidebar: Rect::default(),
                editor: Rect::default(),
                agent: Rect::default(),
                bottom: Rect::default(),
                compact: true,
            };
        }

        // Round the default split to the nearest cell and give any remainder
        // to the agent. A manually dragged divider stores an exact cell width.
        let default_editor_width = ((u32::from(main_width) * 55 + 50) / 100) as u16;
        let editor_width = config.editor_width.unwrap_or(default_editor_width).clamp(
            MIN_TERMINAL_WIDTH,
            main_width.saturating_sub(MIN_TERMINAL_WIDTH),
        );
        let agent_width = main_width - editor_width;
        let main_x = area.x.saturating_add(sidebar_width);

        let bottom_height = if visibility.bottom
            && area.height >= MIN_TERMINAL_HEIGHT.saturating_add(MIN_SHELL_PANE_HEIGHT)
        {
            config.bottom_height.clamp(
                MIN_SHELL_PANE_HEIGHT,
                area.height.saturating_sub(MIN_TERMINAL_HEIGHT),
            )
        } else {
            0
        };
        let editor_height = area.height - bottom_height;

        Self {
            sidebar: if visibility.sidebar {
                Rect::new(area.x, area.y, sidebar_width, area.height)
            } else {
                Rect::default()
            },
            editor: Rect::new(main_x, area.y, editor_width, editor_height),
            agent: Rect::new(
                main_x.saturating_add(editor_width),
                area.y,
                agent_width,
                area.height,
            ),
            bottom: if visibility.bottom && bottom_height >= MIN_SHELL_PANE_HEIGHT {
                Rect::new(
                    main_x,
                    area.y.saturating_add(editor_height),
                    editor_width,
                    bottom_height,
                )
            } else {
                Rect::default()
            },
            compact: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_55_45_post_sidebar_geometry() {
        let area = Rect::new(0, 0, 160, 50);
        let layout = WorkbenchLayout::calculate(area, WorkbenchLayoutConfig::default());

        assert_eq!(layout.sidebar.width, 24);
        assert_eq!((layout.editor.width, layout.agent.width), (75, 61));
        assert_eq!(layout.bottom.height, 12);
        assert_eq!(layout.agent.height, 50);
        assert_eq!(layout.bottom.y, 38);
        assert_eq!(layout.bottom.x, layout.editor.x);
        assert_eq!(layout.bottom.width, layout.editor.width);
    }

    #[test]
    fn exhaustive_small_viewports_are_contained_and_visible_terminals_are_nonzero() {
        let config = WorkbenchLayoutConfig::default();
        for width in 0..=80 {
            for height in 0..=24 {
                let area = Rect::new(7, 9, width, height);
                for visibility in [
                    WorkbenchVisibility {
                        sidebar: false,
                        bottom: false,
                    },
                    WorkbenchVisibility {
                        sidebar: true,
                        bottom: false,
                    },
                    WorkbenchVisibility {
                        sidebar: false,
                        bottom: true,
                    },
                    WorkbenchVisibility {
                        sidebar: true,
                        bottom: true,
                    },
                ] {
                    let layout = WorkbenchLayout::calculate_visible(area, config, visibility);
                    for pane in [layout.sidebar, layout.editor, layout.agent, layout.bottom] {
                        assert!(pane.right() <= area.right());
                        assert!(pane.bottom() <= area.bottom());
                    }
                    for pane in [layout.editor, layout.agent] {
                        if pane.width != 0 || pane.height != 0 {
                            assert!(pane.width >= MIN_TERMINAL_WIDTH);
                            assert!(pane.height >= MIN_TERMINAL_HEIGHT);
                        }
                    }
                    if layout.bottom.width != 0 || layout.bottom.height != 0 {
                        assert!(layout.bottom.width >= MIN_TERMINAL_WIDTH);
                        assert!(layout.bottom.height >= MIN_SHELL_PANE_HEIGHT);
                    }
                }
            }
        }
    }

    #[test]
    fn editor_agent_width_is_cell_exact_after_manual_drag() {
        let config = WorkbenchLayoutConfig {
            editor_width: Some(77),
            ..WorkbenchLayoutConfig::default()
        };
        let layout = WorkbenchLayout::calculate(Rect::new(0, 0, 160, 40), config);
        assert_eq!((layout.editor.width, layout.agent.width), (77, 59));
    }

    #[test]
    fn shell_tab_row_requires_five_pane_rows() {
        let config = WorkbenchLayoutConfig {
            bottom_height: MIN_SHELL_PANE_HEIGHT,
            ..WorkbenchLayoutConfig::default()
        };
        let hidden = WorkbenchLayout::calculate_visible(
            Rect::new(0, 0, 120, 8),
            config,
            WorkbenchVisibility {
                sidebar: false,
                bottom: true,
            },
        );
        assert_eq!(hidden.bottom, Rect::default());

        let visible = WorkbenchLayout::calculate_visible(
            Rect::new(0, 0, 120, 9),
            config,
            WorkbenchVisibility {
                sidebar: false,
                bottom: true,
            },
        );
        assert_eq!(visible.editor.height, MIN_TERMINAL_HEIGHT);
        assert_eq!(visible.bottom.height, MIN_SHELL_PANE_HEIGHT);
    }

    #[test]
    fn eighty_by_twenty_four_is_safe() {
        let config = WorkbenchLayoutConfig::default();
        let layout = WorkbenchLayout::calculate(Rect::new(0, 0, 80, 24), config);
        assert!(!layout.compact);
        assert_eq!((layout.editor.width, layout.agent.width), (31, 25));
        assert_eq!((layout.editor.height, layout.bottom.height), (12, 12));
    }
}
