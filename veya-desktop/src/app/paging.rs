//! Viewport-sized mouse-wheel paging for the two primary history lists.
//! Transform line-wheel events before the scrollable consumes them; wrapping it
//! in a mouse area after the fact cannot override an already-captured event.

use iced::advanced::widget::{tree, Operation, Tree};
use iced::advanced::{layout, overlay, renderer, Clipboard, Layout, Shell, Widget};
use iced::{event, mouse, Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector};

pub(super) fn page_scroll<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    Element::new(PageScroll {
        content: content.into(),
    })
}

struct PageScroll<'a, Message> {
    content: Element<'a, Message>,
}

impl<Message> Widget<Message, Theme, Renderer> for PageScroll<'_, Message> {
    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content.as_widget().layout(tree, renderer, limits)
    }

    fn operate(
        &self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget()
            .operate(tree, layout, renderer, operation);
    }

    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) -> event::Status {
        let event = match event {
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let delta = match layout.bounds().intersection(viewport) {
                    Some(visible) if cursor.is_over(visible) => {
                        wheel_delta(delta, page_height(layout, visible.height))
                    }
                    _ => delta,
                };
                Event::Mouse(mouse::Event::WheelScrolled { delta })
            }
            other => other,
        };
        self.content.as_widget_mut().on_event(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        )
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        translation: Vector,
    ) -> Option<overlay::Element<'a, Message, Theme, Renderer>> {
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, translation)
    }
}

/// Equal-height rows page by whole rows. Detailed history can have mixed row
/// heights; in that case use the actual viewport rather than guessing a stride.
fn page_height(layout: Layout<'_>, visible_height: f32) -> f32 {
    let Some(content) = layout.children().next() else {
        return visible_height;
    };
    let mut rows = content.children();
    let (Some(first), Some(second)) = (rows.next(), rows.next()) else {
        return visible_height;
    };
    let first = first.bounds();
    let mut previous = second.bounds();
    let stride = previous.y - first.y;
    if stride <= 0.0 || (previous.height - first.height).abs() > 0.5 {
        return visible_height;
    }
    for row in rows {
        let bounds = row.bounds();
        if (bounds.height - first.height).abs() > 0.5
            || (bounds.y - previous.y - stride).abs() > 0.5
        {
            return visible_height;
        }
        previous = bounds;
    }
    row_aligned_page(visible_height, first.height, stride)
}

fn row_aligned_page(visible_height: f32, row_height: f32, stride: f32) -> f32 {
    if !visible_height.is_finite() || !stride.is_finite() || stride <= 0.0 {
        return visible_height;
    }
    // A page of N complete rows has only N-1 gaps. Include that final gap in
    // the distance to the next row, but not in the complete-row count.
    let gap = (stride - row_height).max(0.0);
    let rows = ((visible_height + gap + 0.01) / stride).floor();
    if rows >= 1.0 {
        rows * stride
    } else {
        visible_height
    }
}

fn wheel_delta(delta: mouse::ScrollDelta, page: f32) -> mouse::ScrollDelta {
    match delta {
        mouse::ScrollDelta::Lines { x, y }
            if y != 0.0 && y.is_finite() && x.is_finite() && page.is_finite() && page > 0.0 =>
        {
            // Winit supplies raw notches (including fractional/multiple ones).
            // Pixels bypass Iced's fixed 60-pixel line multiplier. Preserve its
            // ordinary horizontal distance and leave touchpad pixels unchanged.
            mouse::ScrollDelta::Pixels {
                x: x * 60.0,
                y: y * page,
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::{column, scrollable, Space};

    fn scroll_offsets(
        row_height: f32,
        viewport_height: f32,
        count: usize,
        deltas: &[mouse::ScrollDelta],
        cursor: mouse::Cursor,
    ) -> Vec<f32> {
        let rows = (0..count).map(|_| {
            Element::<'_, scrollable::Viewport>::from(Space::new(
                Length::Fill,
                Length::Fixed(row_height),
            ))
        });
        let content = scrollable(column(rows).spacing(2))
            .width(Length::Fill)
            .height(Length::Fill)
            .on_scroll(|viewport| viewport);
        let mut renderer = iced::Renderer::new(crate::font::ui_font(), iced::Pixels(16.0));
        let mut ui = iced_runtime::UserInterface::build(
            page_scroll(content),
            Size::new(200.0, viewport_height),
            iced_runtime::user_interface::Cache::new(),
            &mut renderer,
        );
        let events: Vec<_> = deltas
            .iter()
            .map(|delta| Event::Mouse(mouse::Event::WheelScrolled { delta: *delta }))
            .collect();
        let mut messages = Vec::new();
        let _ = ui.update(
            &events,
            cursor,
            &mut renderer,
            &mut iced::advanced::clipboard::Null,
            &mut messages,
        );
        messages
            .into_iter()
            .map(|viewport| viewport.absolute_offset().y)
            .collect()
    }

    fn notch(y: f32) -> mouse::ScrollDelta {
        mouse::ScrollDelta::Lines { x: 0.0, y }
    }

    fn over_list() -> mouse::Cursor {
        mouse::Cursor::Available(iced::Point::new(50.0, 50.0))
    }

    #[test]
    fn ten_visible_rows_advance_ten_rows_at_different_font_sizes() {
        for height in [20.0, 40.0, 80.0] {
            let stride = height + 2.0;
            let visible = height * 10.0 + 2.0 * 9.0;
            let offsets = scroll_offsets(height, visible, 50, &[notch(-1.0)], over_list());
            assert!((offsets[0] - stride * 10.0).abs() < 0.01);
        }
    }

    #[test]
    fn resizing_changes_the_number_of_rows_per_page() {
        let offsets = scroll_offsets(40.0, 208.0, 50, &[notch(-1.0)], over_list());
        assert!((offsets[0] - 210.0).abs() < 0.01);
    }

    #[test]
    fn consecutive_down_and_up_notches_do_not_double_scroll() {
        let offsets = scroll_offsets(
            40.0,
            418.0,
            50,
            &[notch(-1.0), notch(-1.0), notch(1.0)],
            over_list(),
        );
        assert_eq!(offsets, [420.0, 840.0, 420.0]);
    }

    #[test]
    fn the_last_partial_page_and_the_top_are_clamped() {
        let offsets = scroll_offsets(40.0, 418.0, 12, &[notch(-1.0), notch(1.0)], over_list());
        assert_eq!(offsets, [84.0, 0.0]);
    }

    #[test]
    fn touchpad_pixel_scrolling_is_not_replaced_by_paging() {
        let pixels = mouse::ScrollDelta::Pixels { x: 0.0, y: -23.0 };
        let offsets = scroll_offsets(40.0, 418.0, 50, &[pixels], over_list());
        assert_eq!(offsets, [23.0]);
    }

    #[test]
    fn scrolling_outside_the_list_does_nothing() {
        let cursor = mouse::Cursor::Available(iced::Point::new(250.0, 50.0));
        assert!(scroll_offsets(40.0, 418.0, 50, &[notch(-1.0)], cursor).is_empty());
    }

    #[test]
    fn fractional_notches_remain_fractional_pages() {
        match wheel_delta(notch(-0.25), 420.0) {
            mouse::ScrollDelta::Pixels { y, .. } => assert_eq!(y, -105.0),
            _ => panic!("expected a pixel delta"),
        }
    }
}
