//! Product visual language — charcoal chrome + one blue accent (see UI mockup).
//! Borders follow Kite: 1px `BORDER` on the outer window and every card/panel.
//!
//! Runtime light/dark palette: the active palette is stored in a thread-local
//! `Cell<&'static Palette>`.  Callers use `theme::INK()` / `theme::BG()` etc.
//! Style functions read the palette via `palette()`.

use iced::widget::{button, container, overlay, pick_list, scrollable, text, text_input};
use iced::{Background, Border, Color, Padding, Theme};
use std::cell::Cell;

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

/// A complete color palette used by every style function and UI helper.
///
/// Each field corresponds to a former `pub const`; the accessor functions below
/// (`BG()`, `INK()`, …) resolve to the active palette at call time.
#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color,
    pub sidebar: Color,
    pub surface: Color,
    pub elevated: Color,
    pub border: Color,
    pub window_border: Color,
    pub border_subtle: Color,
    pub ink: Color,
    pub muted: Color,
    pub faint: Color,
    pub accent: Color,
    pub accent_soft: Color,
    pub accent_border: Color,
    pub danger: Color,
    pub ok: Color,
    pub warn: Color,
    pub chip_bg: Color,
    /// True for the light palette; lets a few widgets pick light-only variants.
    pub light: bool,
    /// Drop shadow under floating panels.
    pub shadow: Color,
    /// Dim layer behind modal dialogs.
    pub backdrop: Color,
    /// Fill of floating menus (context menu, pick-list popup).
    pub overlay: Color,
    /// Hover fill of destructive menu items.
    pub danger_soft: Color,
    /// Tinted fill for "on" status pills.
    pub ok_soft: Color,
    /// Tinted fill for "off" status pills.
    pub neutral_soft: Color,
}

/// Dark palette — refined deep navy/charcoal (original mockup colours).
pub static DARK: Palette = Palette {
    bg: Color::from_rgb(0.06, 0.08, 0.11),
    sidebar: Color::from_rgb(0.05, 0.07, 0.09),
    surface: Color::from_rgb(0.08, 0.11, 0.15),
    elevated: Color::from_rgb(0.11, 0.15, 0.20),
    border: Color::from_rgba(0.24, 0.35, 0.48, 0.28),
    window_border: Color::from_rgba(0.35, 0.48, 0.65, 0.35),
    border_subtle: Color::from_rgba(0.18, 0.26, 0.36, 0.25),
    ink: Color::from_rgb(0.95, 0.97, 0.99),
    muted: Color::from_rgb(0.60, 0.66, 0.75),
    faint: Color::from_rgb(0.38, 0.44, 0.52),
    accent: Color::from_rgb(0.18, 0.48, 0.96),
    accent_soft: Color::from_rgb(0.09, 0.18, 0.30),
    accent_border: Color::from_rgb(0.24, 0.55, 0.98),
    danger: Color::from_rgb(0.97, 0.42, 0.42),
    ok: Color::from_rgb(0.18, 0.82, 0.58),
    warn: Color::from_rgb(0.98, 0.74, 0.16),
    chip_bg: Color::from_rgb(0.07, 0.10, 0.14),
    light: false,
    shadow: Color::from_rgba(0.0, 0.0, 0.0, 0.45),
    backdrop: Color::from_rgba(0.01, 0.03, 0.06, 0.62),
    overlay: Color::from_rgb(0.11, 0.15, 0.20),
    danger_soft: Color::from_rgb(0.24, 0.14, 0.17),
    ok_soft: Color::from_rgb(0.08, 0.20, 0.16),
    neutral_soft: Color::from_rgb(0.12, 0.15, 0.19),
};

/// Light palette — cool off-white window, white cards, one blue accent.
///
/// Layering (lightest to darkest): `surface` white cards, `bg` window,
/// `chip_bg` inset fields, `elevated` hover fill. Text colors are chosen to
/// keep at least 4.5:1 contrast on `bg` and white (`faint`, used for
/// placeholders and idle icons, keeps 3.5:1). `light_palette_text_is_readable`
/// guards these ratios.
pub static LIGHT: Palette = Palette {
    bg: Color::from_rgb(0.965, 0.970, 0.978),
    sidebar: Color::from_rgb(0.930, 0.940, 0.955),
    surface: Color::from_rgb(1.0, 1.0, 1.0),
    elevated: Color::from_rgb(0.905, 0.920, 0.943),
    border: Color::from_rgba(0.48, 0.54, 0.63, 0.42),
    window_border: Color::from_rgba(0.40, 0.47, 0.56, 0.50),
    border_subtle: Color::from_rgba(0.50, 0.56, 0.64, 0.30),
    ink: Color::from_rgb(0.10, 0.12, 0.16),
    muted: Color::from_rgb(0.37, 0.42, 0.49),
    faint: Color::from_rgb(0.45, 0.50, 0.57),
    accent: Color::from_rgb(0.13, 0.41, 0.88),
    accent_soft: Color::from_rgb(0.87, 0.92, 0.99),
    accent_border: Color::from_rgb(0.22, 0.52, 0.96),
    danger: Color::from_rgb(0.78, 0.20, 0.20),
    ok: Color::from_rgb(0.05, 0.47, 0.30),
    warn: Color::from_rgb(0.60, 0.38, 0.02),
    chip_bg: Color::from_rgb(0.938, 0.946, 0.962),
    light: true,
    shadow: Color::from_rgba(0.10, 0.14, 0.22, 0.20),
    backdrop: Color::from_rgba(0.10, 0.13, 0.20, 0.38),
    overlay: Color::from_rgb(1.0, 1.0, 1.0),
    danger_soft: Color::from_rgb(0.99, 0.91, 0.91),
    ok_soft: Color::from_rgb(0.87, 0.96, 0.92),
    neutral_soft: Color::from_rgb(0.92, 0.93, 0.95),
};

thread_local! {
    static ACTIVE: Cell<&'static Palette> = Cell::new(&LIGHT);
}

/// Resolve the active palette (dark or light) at runtime.
pub fn palette() -> &'static Palette {
    ACTIVE.with(|c| c.get())
}

/// Set the active palette — called from [`App::theme`] / message handlers.
pub fn set_palette(p: &'static Palette) {
    ACTIVE.with(|c| c.set(p));
}

// ---------------------------------------------------------------------------
// Color accessors (drop-in replacements for the former `pub const` values)
// ---------------------------------------------------------------------------

pub fn BG() -> Color {
    palette().bg
}
pub fn SIDEBAR() -> Color {
    palette().sidebar
}
pub fn SURFACE() -> Color {
    palette().surface
}
pub fn ELEVATED() -> Color {
    palette().elevated
}
pub fn BORDER() -> Color {
    palette().border
}
pub fn WINDOW_BORDER() -> Color {
    palette().window_border
}
pub fn BORDER_SUBTLE() -> Color {
    palette().border_subtle
}
pub fn INK() -> Color {
    palette().ink
}
pub fn MUTED() -> Color {
    palette().muted
}
pub fn FAINT() -> Color {
    palette().faint
}
pub fn ACCENT() -> Color {
    palette().accent
}
pub fn ACCENT_SOFT() -> Color {
    palette().accent_soft
}
pub fn ACCENT_BORDER() -> Color {
    palette().accent_border
}
pub fn DANGER() -> Color {
    palette().danger
}
pub fn OK() -> Color {
    palette().ok
}
pub fn WARN() -> Color {
    palette().warn
}
pub fn CHIP_BG() -> Color {
    palette().chip_bg
}
pub fn is_light() -> bool {
    palette().light
}
pub fn OK_SOFT() -> Color {
    palette().ok_soft
}
pub fn NEUTRAL_SOFT() -> Color {
    palette().neutral_soft
}
/// Text and icons drawn on a solid `ACCENT()` background; identical in both palettes.
pub fn ON_ACCENT() -> Color {
    ON_ACCENT_COLOR
}
const ON_ACCENT_COLOR: Color = Color::from_rgb(0.97, 0.98, 1.0);

// ---------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------

pub fn bg(c: Color) -> Background {
    Background::Color(c)
}

pub fn gradient(start: Color, end: Color) -> Background {
    iced::gradient::Linear::new(iced::Radians(1.2))
        .add_stop(0.0, start)
        .add_stop(1.0, end)
        .into()
}

pub fn stroke(color: Color, width: f32, radius: f32) -> Border {
    Border {
        color,
        width,
        radius: radius.into(),
    }
}

// ---------------------------------------------------------------------------
// Container styles
// ---------------------------------------------------------------------------

/// Outer window shell: 1px `WINDOW_BORDER`, soft round (mockup window).
pub fn shell_style(_t: &Theme) -> container::Style {
    container::Style {
        background: Some(bg(BG())),
        text_color: Some(INK()),
        border: stroke(WINDOW_BORDER(), 1.0, 12.0),
        ..Default::default()
    }
}

/// Sidebar: darker charcoal; right edge is a [`vdivider`].
pub fn sidebar_style(_t: &Theme) -> container::Style {
    let p = palette();
    container::Style {
        background: Some(gradient(
            Color::from_rgb(
                (p.sidebar.r + p.bg.r) / 2.0,
                (p.sidebar.g + p.bg.g) / 2.0,
                (p.sidebar.b + p.bg.b) / 2.0,
            ),
            p.sidebar,
        )),
        text_color: Some(INK()),
        ..Default::default()
    }
}

/// Top chrome under the window edge — bottom edge is a [`hdivider`].
pub fn topbar_style(_t: &Theme) -> container::Style {
    container::Style {
        background: Some(gradient(
            Color::from_rgb(
                (BG().r + SURFACE().r) / 2.0,
                (BG().g + SURFACE().g) / 2.0,
                (BG().b + SURFACE().b) / 2.0,
            ),
            BG(),
        )),
        text_color: Some(INK()),
        ..Default::default()
    }
}

/// Card / panel: 1px border + radius 12 (mockup cards).
pub fn panel_style(_t: &Theme) -> container::Style {
    container::Style {
        background: Some(gradient(SURFACE(), BG())),
        text_color: Some(INK()),
        border: stroke(BORDER(), 1.0, 12.0),
        ..Default::default()
    }
}

pub fn modal_backdrop_style(_t: &Theme) -> container::Style {
    container::Style {
        background: Some(bg(palette().backdrop)),
        ..Default::default()
    }
}

pub fn modal_panel_style(_t: &Theme) -> container::Style {
    container::Style {
        background: Some(bg(SURFACE())),
        text_color: Some(INK()),
        border: stroke(WINDOW_BORDER(), 1.0, 14.0),
        ..Default::default()
    }
}

pub fn modal_content_style(_t: &Theme) -> container::Style {
    container::Style {
        background: Some(bg(CHIP_BG())),
        text_color: Some(INK()),
        border: stroke(BORDER_SUBTLE(), 1.0, 9.0),
        ..Default::default()
    }
}

/// Inset well for secondary context cards and type chips.
pub fn inset_style(_t: &Theme) -> container::Style {
    container::Style {
        background: Some(bg(CHIP_BG())),
        text_color: Some(INK()),
        border: stroke(BORDER(), 1.0, 10.0),
        ..Default::default()
    }
}

/// Circular glowing Hero chip in detail header
#[allow(dead_code)]
pub fn hero_chip_style(_t: &Theme) -> container::Style {
    container::Style {
        background: Some(gradient(
            Color::from_rgb(0.12, 0.28, 0.48),
            Color::from_rgb(0.08, 0.18, 0.32),
        )),
        text_color: Some(INK()),
        border: stroke(Color::from_rgba(0.24, 0.55, 0.98, 0.35), 1.0, 25.0),
        ..Default::default()
    }
}

/// Search field outer shell (icon + input + keycap).
pub fn search_shell_style(_t: &Theme) -> container::Style {
    container::Style {
        background: Some(bg(CHIP_BG())),
        text_color: Some(INK()),
        border: stroke(BORDER(), 1.0, 10.0),
        ..Default::default()
    }
}

/// 1px vertical split (sidebar | main).
pub fn vdivider<'a, Message>() -> container::Container<'a, Message, Theme, iced::Renderer>
where
    Message: 'a,
{
    use iced::{widget::Space, Length};
    container(Space::new(Length::Fixed(1.0), Length::Fill))
        .width(Length::Fixed(1.0))
        .height(Length::Fill)
        .style(|_t: &Theme| container::Style {
            background: Some(bg(BORDER_SUBTLE())),
            ..Default::default()
        })
}

/// 1px horizontal split (topbar | body, list rows).
pub fn hdivider<'a, Message>() -> container::Container<'a, Message, Theme, iced::Renderer>
where
    Message: 'a,
{
    use iced::{widget::Space, Length};
    container(Space::new(Length::Fill, Length::Fixed(1.0)))
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .style(|_t: &Theme| container::Style {
            background: Some(bg(BORDER_SUBTLE())),
            ..Default::default()
        })
}

// ---------------------------------------------------------------------------
// Button styles
// ---------------------------------------------------------------------------

pub fn card_button(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    move |_t: &Theme, status: button::Status| {
        let p = palette();
        let (bg_start, bg_end, border_c, border_w) = if selected {
            (
                Color::from_rgb(
                    (p.accent_soft.r + p.surface.r) / 2.0,
                    (p.accent_soft.g + p.surface.g) / 2.0,
                    (p.accent_soft.b + p.surface.b) / 2.0,
                ),
                p.accent_soft,
                p.accent_border,
                1.5,
            )
        } else {
            match status {
                button::Status::Hovered => (
                    Color::from_rgb(
                        (p.elevated.r + p.surface.r) / 2.0,
                        (p.elevated.g + p.surface.g) / 2.0,
                        (p.elevated.b + p.surface.b) / 2.0,
                    ),
                    p.elevated,
                    Color::from_rgba(p.border.r, p.border.g, p.border.b, 0.3),
                    1.0,
                ),
                _ => (
                    Color::TRANSPARENT,
                    Color::TRANSPARENT,
                    Color::TRANSPARENT,
                    0.0,
                ),
            }
        };
        button::Style {
            background: Some(if selected || matches!(status, button::Status::Hovered) {
                gradient(bg_start, bg_end)
            } else {
                bg(Color::TRANSPARENT)
            }),
            text_color: INK(),
            border: stroke(border_c, border_w, 10.0),
            ..Default::default()
        }
    }
}

/// Quick-panel card button: like [`card_button`] but also supports an
/// alternating row tint (`alt`) and a left-edge red marker for items that
/// have been pasted (`used`).
///
/// - `alt = true` applies a very subtle surface tint so adjacent rows are
///   visually distinguishable even without hover.
/// - `used = true` draws a 3 px red left border to signal "already pasted".
pub fn card_button_quick(
    selected: bool,
    alt: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    move |_t: &Theme, status: button::Status| {
        let p = palette();

        let (bg_start, bg_end, border_c, border_w) = if selected {
            (
                Color::from_rgb(
                    (p.accent_soft.r + p.surface.r) / 2.0,
                    (p.accent_soft.g + p.surface.g) / 2.0,
                    (p.accent_soft.b + p.surface.b) / 2.0,
                ),
                p.accent_soft,
                p.accent_border,
                1.5,
            )
        } else {
            match status {
                button::Status::Hovered => (
                    Color::from_rgb(
                        (p.elevated.r + p.surface.r) / 2.0,
                        (p.elevated.g + p.surface.g) / 2.0,
                        (p.elevated.b + p.surface.b) / 2.0,
                    ),
                    p.elevated,
                    Color::from_rgba(p.border.r, p.border.g, p.border.b, 0.3),
                    1.0,
                ),
                _ => {
                    // Non-hovered, non-selected: apply alternating tint. White on a near-white
                    // window is invisible, so the light palette shades with its inset grey.
                    if alt {
                        let tint = if p.light {
                            Color { a: 0.85, ..p.chip_bg }
                        } else {
                            Color { a: 0.45, ..p.surface }
                        };
                        (tint, tint, Color::TRANSPARENT, 0.0)
                    } else {
                        (
                            Color::TRANSPARENT,
                            Color::TRANSPARENT,
                            Color::TRANSPARENT,
                            0.0,
                        )
                    }
                }
            }
        };

        button::Style {
            background: Some(
                if selected || matches!(status, button::Status::Hovered) || alt {
                    gradient(bg_start, bg_end)
                } else {
                    bg(Color::TRANSPARENT)
                },
            ),
            text_color: INK(),
            border: stroke(border_c, border_w, 10.0),
            ..Default::default()
        }
    }
}

/// Narrow red bar at the left edge of a quick-paste row that has already been pasted.
pub fn used_marker_style(_t: &Theme) -> container::Style {
    container::Style {
        background: Some(bg(DANGER())),
        border: stroke(Color::TRANSPARENT, 0.0, 2.0),
        ..Default::default()
    }
}

#[allow(dead_code)]
pub fn ghost_button(_t: &Theme, status: button::Status) -> button::Style {
    let (bg_c, border_c, text_c) = match status {
        button::Status::Hovered => (ELEVATED(), BORDER(), INK()),
        button::Status::Pressed => (CHIP_BG(), BORDER(), INK()),
        _ => (Color::TRANSPARENT, Color::TRANSPARENT, MUTED()),
    };
    button::Style {
        background: Some(bg(bg_c)),
        text_color: text_c,
        border: stroke(border_c, 1.0, 8.0),
        ..Default::default()
    }
}

/// Action button inside toolbar or content card (quick actions).
pub fn action_button(_t: &Theme, status: button::Status) -> button::Style {
    let p = palette();
    let (bg_c, text_c, border_c) = match status {
        button::Status::Hovered => (
            Color::from_rgb(
                (p.elevated.r + p.surface.r) / 2.0 + 0.02,
                (p.elevated.g + p.surface.g) / 2.0 + 0.02,
                (p.elevated.b + p.surface.b) / 2.0 + 0.02,
            ),
            INK(),
            Color::from_rgba(p.border.r, p.border.g, p.border.b, 0.4),
        ),
        button::Status::Pressed => (CHIP_BG(), INK(), BORDER()),
        _ => (
            Color::from_rgb(p.elevated.r, p.elevated.g, p.elevated.b),
            MUTED(),
            Color::from_rgba(p.border.r, p.border.g, p.border.b, 0.3),
        ),
    };
    button::Style {
        background: Some(bg(bg_c)),
        text_color: text_c,
        border: stroke(border_c, 1.0, 6.0),
        ..Default::default()
    }
}

pub fn context_menu_panel(_t: &Theme) -> container::Style {
    let p = palette();
    container::Style {
        background: Some(bg(p.overlay)),
        text_color: Some(INK()),
        border: stroke(WINDOW_BORDER(), 1.0, 9.0),
        shadow: iced::Shadow {
            color: p.shadow,
            offset: iced::Vector::new(0.0, 8.0),
            blur_radius: 18.0,
        },
    }
}

pub fn context_menu_item(danger: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_t, status| button::Style {
        background: match status {
            button::Status::Hovered => Some(bg(if danger {
                palette().danger_soft
            } else {
                ACCENT_SOFT()
            })),
            button::Status::Pressed => Some(bg(CHIP_BG())),
            _ => None,
        },
        text_color: if danger { DANGER() } else { INK() },
        border: stroke(Color::TRANSPARENT, 0.0, 6.0),
        ..Default::default()
    }
}

pub fn primary_button(_t: &Theme, status: button::Status) -> button::Style {
    let p = palette();
    let (bg_c, text_c) = match status {
        button::Status::Hovered => (
            Color::from_rgb(
                (p.accent.r + 0.08).min(1.0),
                (p.accent.g + 0.08).min(1.0),
                (p.accent.b + 0.04).min(1.0),
            ),
            ON_ACCENT(),
        ),
        button::Status::Pressed => (
            Color::from_rgb(
                (p.accent.r - 0.02).max(0.0),
                (p.accent.g - 0.06).max(0.0),
                (p.accent.b - 0.08).max(0.0),
            ),
            ON_ACCENT(),
        ),
        _ => (ACCENT(), ON_ACCENT()),
    };
    button::Style {
        background: Some(bg(bg_c)),
        text_color: text_c,
        border: stroke(ACCENT_BORDER(), 1.0, 8.0),
        ..Default::default()
    }
}

pub fn danger_button(_t: &Theme, status: button::Status) -> button::Style {
    let (bg_c, text_c) = match status {
        button::Status::Hovered => (ELEVATED(), DANGER()),
        _ => (Color::TRANSPARENT, DANGER()),
    };
    button::Style {
        background: Some(bg(bg_c)),
        text_color: text_c,
        border: stroke(BORDER(), 1.0, 8.0),
        ..Default::default()
    }
}

pub fn nav_button(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    move |_t: &Theme, status: button::Status| {
        let p = palette();
        let (bg_c, text_c) = if active {
            (p.accent_soft, INK())
        } else {
            match status {
                button::Status::Hovered => (SURFACE(), INK()),
                _ => (Color::TRANSPARENT, MUTED()),
            }
        };
        button::Style {
            background: Some(if active {
                gradient(
                    Color::from_rgb(
                        (p.accent_soft.r + p.accent.r) / 2.0,
                        (p.accent_soft.g + p.accent.g) / 2.0,
                        (p.accent_soft.b + p.accent.b) / 2.0,
                    ),
                    p.accent_soft,
                )
            } else {
                bg(bg_c)
            }),
            text_color: text_c,
            border: stroke(
                if active {
                    Color::from_rgba(p.accent_border.r, p.accent_border.g, p.accent_border.b, 0.5)
                } else {
                    Color::TRANSPARENT
                },
                1.0,
                10.0,
            ),
            ..Default::default()
        }
    }
}

pub fn chip_button(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    move |_t: &Theme, status: button::Status| {
        let (bg_c, border_c, text_c) = if selected {
            (ACCENT(), ACCENT_BORDER(), ON_ACCENT())
        } else {
            match status {
                button::Status::Hovered => (ELEVATED(), BORDER(), INK()),
                _ => (CHIP_BG(), BORDER(), MUTED()),
            }
        };
        button::Style {
            background: Some(bg(bg_c)),
            text_color: text_c,
            border: stroke(border_c, 1.0, 8.0),
            ..Default::default()
        }
    }
}

/// History toolbar filter button: active is bright pill matching mockup.
pub fn toolbar_filter_button(
    selected: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    move |_t: &Theme, status: button::Status| {
        if selected {
            button::Style {
                background: Some(bg(ACCENT())),
                text_color: ON_ACCENT(),
                border: stroke(ACCENT_BORDER(), 1.0, 8.0),
                ..Default::default()
            }
        } else {
            let p = palette();
            let color = match status {
                button::Status::Hovered => Color::from_rgb(
                    (p.elevated.r + p.surface.r) / 2.0,
                    (p.elevated.g + p.surface.g) / 2.0,
                    (p.elevated.b + p.surface.b) / 2.0,
                ),
                button::Status::Pressed => CHIP_BG(),
                _ => Color::TRANSPARENT,
            };
            button::Style {
                background: Some(bg(color)),
                text_color: MUTED(),
                border: stroke(Color::TRANSPARENT, 1.0, 8.0),
                ..Default::default()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pick list / overlay
// ---------------------------------------------------------------------------

pub fn sort_picker(_theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    pick_list::Style {
        text_color: if matches!(
            status,
            pick_list::Status::Hovered | pick_list::Status::Opened
        ) {
            INK()
        } else {
            MUTED()
        },
        placeholder_color: FAINT(),
        handle_color: MUTED(),
        background: bg(
            if matches!(
                status,
                pick_list::Status::Hovered | pick_list::Status::Opened
            ) {
                ELEVATED()
            } else {
                CHIP_BG()
            },
        ),
        border: stroke(BORDER(), 1.0, 8.0),
    }
}

pub fn sort_menu(_theme: &Theme) -> overlay::menu::Style {
    overlay::menu::Style {
        background: bg(palette().overlay),
        border: stroke(BORDER(), 1.0, 8.0),
        text_color: INK(),
        selected_text_color: INK(),
        selected_background: bg(ACCENT_SOFT()),
    }
}

// ---------------------------------------------------------------------------
// Buttons (misc)
// ---------------------------------------------------------------------------

/// Elevated control (top-bar pause).
pub fn elevated_button(_t: &Theme, status: button::Status) -> button::Style {
    let (bg_c, border_c, text_c) = match status {
        button::Status::Hovered => (ELEVATED(), BORDER(), INK()),
        button::Status::Pressed => (CHIP_BG(), BORDER(), INK()),
        _ => (SURFACE(), BORDER(), INK()),
    };
    button::Style {
        background: Some(bg(bg_c)),
        text_color: text_c,
        border: stroke(border_c, 1.0, 10.0),
        ..Default::default()
    }
}

/// Window control glyph buttons (— □ ×).
pub fn win_button(_t: &Theme, status: button::Status) -> button::Style {
    let (bg_c, text_c) = match status {
        button::Status::Hovered => (ELEVATED(), INK()),
        button::Status::Pressed => (CHIP_BG(), INK()),
        _ => (Color::TRANSPARENT, FAINT()),
    };
    button::Style {
        background: Some(bg(bg_c)),
        text_color: text_c,
        border: stroke(Color::TRANSPARENT, 1.0, 6.0),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

pub fn input_style(_t: &Theme, status: text_input::Status) -> text_input::Style {
    let (border_c, bg_c) = match status {
        text_input::Status::Focused => (ACCENT(), Color::TRANSPARENT),
        _ => (Color::TRANSPARENT, Color::TRANSPARENT),
    };
    text_input::Style {
        background: bg(bg_c),
        border: stroke(border_c, 1.0, 8.0),
        icon: FAINT(),
        placeholder: FAINT(),
        value: INK(),
        selection: ACCENT_SOFT(),
    }
}

/// Bordered number field for the settings page. [`input_style`] only draws a frame while
/// focused, which leaves a standalone number box looking like loose text.
pub fn field_style(_t: &Theme, status: text_input::Status) -> text_input::Style {
    let (border_c, bg_c, value_c) = match status {
        text_input::Status::Focused => (ACCENT(), SURFACE(), INK()),
        text_input::Status::Hovered => (ACCENT_BORDER(), CHIP_BG(), INK()),
        text_input::Status::Disabled => (BORDER_SUBTLE(), CHIP_BG(), FAINT()),
        _ => (BORDER(), CHIP_BG(), INK()),
    };
    text_input::Style {
        background: bg(bg_c),
        border: stroke(border_c, 1.0, 8.0),
        icon: FAINT(),
        placeholder: FAINT(),
        value: value_c,
        selection: ACCENT_SOFT(),
    }
}

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

pub fn section_label<'a>(s: impl iced::widget::text::IntoFragment<'a>) -> text::Text<'a, Theme> {
    text(s)
        .size(13)
        .color(MUTED())
        .font(crate::font::name_font())
}

/// Container chrome matching a quiet chip (sort dropdown / view toggle).
pub fn chip_container(_t: &Theme) -> container::Style {
    container::Style {
        background: Some(bg(CHIP_BG())),
        text_color: Some(MUTED()),
        border: stroke(BORDER(), 1.0, 8.0),
        ..Default::default()
    }
}

pub fn scroll_style(_t: &Theme, status: scrollable::Status) -> scrollable::Style {
    let thumb = if matches!(status, scrollable::Status::Active) {
        if is_light() {
            // BORDER is too faint to find on a white list.
            Color { a: 0.40, ..MUTED() }
        } else {
            BORDER()
        }
    } else {
        MUTED()
    };
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            color: thumb,
            border: stroke(Color::TRANSPARENT, 0.0, 3.0),
        },
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
    }
}

pub fn meta<'a>(s: impl iced::widget::text::IntoFragment<'a>) -> text::Text<'a, Theme> {
    text(s).size(12).color(MUTED())
}

pub fn body<'a>(s: impl iced::widget::text::IntoFragment<'a>) -> text::Text<'a, Theme> {
    text(s).size(13).color(INK())
}

// ---------------------------------------------------------------------------
// Padding
// ---------------------------------------------------------------------------

pub fn pad_narrow() -> Padding {
    Padding {
        top: 8.0,
        right: 12.0,
        bottom: 8.0,
        left: 12.0,
    }
}

#[allow(dead_code)]
pub fn pad_card() -> Padding {
    Padding {
        top: 10.0,
        right: 14.0,
        bottom: 10.0,
        left: 14.0,
    }
}

pub fn pad_panel() -> Padding {
    Padding {
        top: 16.0,
        right: 16.0,
        bottom: 16.0,
        left: 16.0,
    }
}

pub fn pad(top: f32, right: f32, bottom: f32, left: f32) -> Padding {
    Padding {
        top,
        right,
        bottom,
        left,
    }
}

// ---------------------------------------------------------------------------
// Timeline spine
// ---------------------------------------------------------------------------

/// Timeline spine segment (vertical line).
pub fn spine_line<'a, Message>(
    height: f32,
) -> container::Container<'a, Message, Theme, iced::Renderer>
where
    Message: 'a,
{
    use iced::{widget::Space, Length};
    container(Space::new(Length::Fixed(2.0), Length::Fixed(height)))
        .width(Length::Fixed(2.0))
        .height(Length::Fixed(height))
        .style(|_t: &Theme| container::Style {
            background: Some(bg(Color::from_rgba(
                ACCENT_BORDER().r,
                ACCENT_BORDER().g,
                ACCENT_BORDER().b,
                0.4,
            ))),
            ..Default::default()
        })
}

#[allow(dead_code)]
pub fn flow_line<'a, Message>(
    height: f32,
) -> container::Container<'a, Message, Theme, iced::Renderer>
where
    Message: 'a,
{
    use iced::{widget::Space, Length};
    container(Space::new(Length::Fixed(2.0), Length::Fixed(height)))
        .width(Length::Fixed(2.0))
        .height(Length::Fixed(height))
        .style(|_t: &Theme| container::Style {
            background: Some(bg(ACCENT())),
            ..Default::default()
        })
}

/// Timeline spine fill (rest of the row).
#[allow(dead_code)]
pub fn spine_fill<'a, Message>() -> container::Container<'a, Message, Theme, iced::Renderer>
where
    Message: 'a,
{
    use iced::{widget::Space, Length};
    container(Space::new(Length::Fixed(2.0), Length::Fixed(24.0)))
        .width(Length::Fixed(2.0))
        .height(Length::Fixed(24.0))
        .style(|_t: &Theme| container::Style {
            background: Some(bg(Color::from_rgba(
                ACCENT_BORDER().r,
                ACCENT_BORDER().g,
                ACCENT_BORDER().b,
                0.4,
            ))),
            ..Default::default()
        })
}

/// Timeline node dot.
pub fn spine_dot<'a, Message>(
    active: bool,
) -> container::Container<'a, Message, Theme, iced::Renderer>
where
    Message: 'a,
{
    use iced::{widget::Space, Length};
    let color = if active { ACCENT() } else { BORDER() };
    container(Space::new(Length::Fixed(10.0), Length::Fixed(10.0)))
        .width(Length::Fixed(10.0))
        .height(Length::Fixed(10.0))
        .style(move |_t: &Theme| container::Style {
            background: Some(bg(color)),
            border: stroke(ACCENT_BORDER(), 1.5, 5.0),
            ..Default::default()
        })
}

/// Small keycap (Ctrl / K).
pub fn keycap<'a, Message>(
    label: &'a str,
) -> container::Container<'a, Message, Theme, iced::Renderer>
where
    Message: 'a,
{
    container(
        text(label)
            .size(11)
            .color(MUTED())
            .font(crate::font::name_font()),
    )
    .padding(pad(3.0, 7.0, 3.0, 7.0))
    .style(|_t: &Theme| container::Style {
        background: Some(bg(ELEVATED())),
        text_color: Some(MUTED()),
        border: stroke(BORDER(), 1.0, 5.0),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: Color) -> f32 {
        let lin = |v: f32| {
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
    }

    fn contrast(a: Color, b: Color) -> f32 {
        let (hi, lo) = {
            let (x, y) = (luminance(a), luminance(b));
            if x > y {
                (x, y)
            } else {
                (y, x)
            }
        };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn light_palette_text_is_readable() {
        let p = &LIGHT;
        for ground in [p.bg, p.surface] {
            for (name, fg) in [
                ("ink", p.ink),
                ("muted", p.muted),
                ("danger", p.danger),
                ("ok", p.ok),
                ("warn", p.warn),
                ("accent", p.accent),
            ] {
                assert!(contrast(fg, ground) >= 4.5, "{name} on {ground:?}");
            }
            assert!(contrast(p.faint, ground) >= 3.5, "faint on {ground:?}");
        }
        assert!(contrast(p.faint, p.chip_bg) >= 3.4, "placeholder in a field");
        assert!(contrast(ON_ACCENT_COLOR, p.accent) >= 4.5, "label on accent");
    }

    #[test]
    fn light_layers_are_distinguishable() {
        let p = &LIGHT;
        assert!(luminance(p.surface) > luminance(p.bg));
        assert!(luminance(p.bg) > luminance(p.chip_bg));
        assert!(luminance(p.chip_bg) > luminance(p.elevated));
        assert!(p.light && !DARK.light);
    }
}
