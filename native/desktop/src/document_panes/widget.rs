//! Attach canvas input to its document before applying legacy tool actions.
use super::Action;
use crate::Message;
use iced::widget::canvas::{self, Geometry, Program};
use iced::{event, mouse, Rectangle, Renderer, Theme};
use std::cell::Cell;

pub(crate) struct Scoped<'a, P> {
    pub inner: P,
    pub key: &'a str,
    pub pane_bounds: &'a Cell<Option<Rectangle>>,
    pub bounds: &'a Cell<Option<Rectangle>>,
}
impl<P: Program<Message>> Program<Message> for Scoped<'_, P> {
    type State = P::State;
    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (event::Status, Option<Message>) {
        self.bounds.set(Some(bounds));
        self.pane_bounds.set(Some(bounds));
        let (status, message) = self.inner.update(state, event, bounds, cursor);
        (
            status,
            message.map(|message| {
                Message::Documents(Action::Input(self.key.into(), Box::new(message)))
            }),
        )
    }
    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        self.bounds.set(Some(bounds));
        self.pane_bounds.set(Some(bounds));
        self.inner.draw(state, renderer, theme, bounds, cursor)
    }
    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        self.inner.mouse_interaction(state, bounds, cursor)
    }
}

pub(crate) struct Empty<'a>(pub &'a Cell<Option<Rectangle>>);
impl Program<Message> for Empty<'_> {
    type State = ();
    fn draw(
        &self,
        _: &(),
        _: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry> {
        self.0.set(Some(bounds));
        Vec::new()
    }
}
