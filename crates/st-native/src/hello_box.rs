//! `<hello-box>` — the M0-08 proof that our own GPUI custom element paints.
//!
//! Deliberately trivial: one rounded quad and one shaped text run, with the
//! quad's fill read from a `color` prop so the prop pipeline
//! (React `setCustomProp` → `CustomElementEntry::sync` → `set_prop`) is proven
//! end to end. `<terminal-grid>` (M2-02) replaces it and this file is deleted.

use gpuix_native::{CustomElement, CustomElementFactory, CustomRenderContext, GpuixView};

/// Element type string React writes as `<hello-box />`.
pub const ELEMENT_TYPE: &str = "hello-box";

const LABEL: &str = "hello-box";
const DEFAULT_COLOR: u32 = 0x3b82f6;

/// Factory for `<hello-box>`.
pub struct HelloBoxFactory;

impl CustomElementFactory for HelloBoxFactory {
    fn element_type(&self) -> &str {
        ELEMENT_TYPE
    }

    fn create(&self, _id: u64) -> Box<dyn CustomElement> {
        Box::new(HelloBox::default())
    }
}

/// The `<hello-box>` element state.
pub struct HelloBox {
    color: gpui::Rgba,
    label: String,
}

impl Default for HelloBox {
    fn default() -> Self {
        Self {
            color: gpui::rgb(DEFAULT_COLOR),
            label: LABEL.to_string(),
        }
    }
}

impl CustomElement for HelloBox {
    fn render(
        &mut self,
        ctx: CustomRenderContext,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<GpuixView>,
    ) -> gpui::AnyElement {
        use gpui::prelude::*;

        let mut el = gpui::div()
            // Stateful id: gpui keys hover/active state and the accessibility
            // node off it, and it must be stable across frames.
            .id(gpui::SharedString::from(format!(
                "__st_hello_box_{}",
                ctx.id
            )))
            .flex()
            .items_center()
            .justify_center()
            .px(gpui::px(18.0))
            .py(gpui::px(12.0))
            .rounded(gpui::px(10.0))
            .bg(self.color)
            .text_color(readable_on(self.color))
            .text_size(gpui::px(14.0));

        // Honour the JSX `style` prop the same way the built-in elements do.
        if let Some(style) = ctx.style {
            el = gpuix_native::apply_interactive_styles(el, style);
        }

        // `ctx.text`, not a bare `.child(str)`: it routes the run through
        // gpuix's selectable-text machinery, so a drag that starts outside the
        // element still sees these glyphs.
        let label = self.label.clone();
        let el = el.child(ctx.text(0, label, None));

        // Publish our painted bounds so `getAutomationTree()` can see this
        // element. Every built-in gpuix element does this via `custom_surface`,
        // which is `pub(crate)`; out-of-tree elements have to call it directly.
        gpuix_native::automation::track_own_bounds(el, ctx.id).into_any_element()
    }

    fn set_prop(&mut self, key: &str, value: serde_json::Value) {
        match key {
            "color" => {
                self.color = value
                    .as_str()
                    .and_then(parse_hex_color)
                    .unwrap_or(gpui::rgb(DEFAULT_COLOR))
            }
            "label" => {
                self.label = value
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(LABEL)
                    .to_string()
            }
            _ => {}
        }
    }

    fn supported_props(&self) -> &'static [&'static str] {
        &["color", "label"]
    }

    fn supported_events(&self) -> &'static [&'static str] {
        &[]
    }

    fn destroy(&mut self) {}
}

/// `#rgb`, `#rrggbb` and `#rrggbbaa`, with or without the leading `#`.
/// Deliberately not a full CSS colour parser — `<terminal-grid>` takes its
/// palette as structured JSON, not as CSS strings.
fn parse_hex_color(raw: &str) -> Option<gpui::Rgba> {
    let hex = raw.trim().trim_start_matches('#');
    let expand = |c: u8| c * 17;
    let nibble = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok();
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();

    let (r, g, b, a) = match hex.len() {
        3 => (
            expand(nibble(0)?),
            expand(nibble(1)?),
            expand(nibble(2)?),
            255,
        ),
        6 => (byte(0)?, byte(2)?, byte(4)?, 255),
        8 => (byte(0)?, byte(2)?, byte(4)?, byte(6)?),
        _ => return None,
    };
    Some(gpui::Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: a as f32 / 255.0,
    })
}

/// Pick black or white text for a background. sRGB relative luminance with the
/// usual 0.5 cut — good enough to keep the label legible on any `color`.
fn readable_on(bg: gpui::Rgba) -> gpui::Rgba {
    let luminance = 0.2126 * bg.r + 0.7152 * bg.g + 0.0722 * bg.b;
    if luminance > 0.5 {
        gpui::rgb(0x101014)
    } else {
        gpui::rgb(0xf5f5f7)
    }
}

#[cfg(test)]
mod tests {
    use super::parse_hex_color;

    #[test]
    fn parses_the_three_accepted_hex_shapes() {
        assert_eq!(parse_hex_color("#f00").unwrap().r, 1.0);
        assert_eq!(parse_hex_color("00ff00").unwrap().g, 1.0);
        assert_eq!(parse_hex_color("#0000ff80").unwrap().b, 1.0);
        assert!(parse_hex_color("rebeccapurple").is_none());
        assert!(parse_hex_color("#ggg").is_none());
    }
}
