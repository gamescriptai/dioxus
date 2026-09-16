use std::cell::Cell;
use wasm_bindgen::{prelude::Closure, JsCast, JsValue};
use web_sys::{window, Event, History, ScrollRestoration, Window};

/// A [`dioxus_history::History`] provider that integrates with a browser via the [History API](https://developer.mozilla.org/en-US/docs/Web/API/History_API).
///
/// # Prefix
/// This [`dioxus_history::History`] supports a prefix, which can be used for web apps that aren't located
/// at the root of their domain.
///
/// Application developers are responsible for ensuring that right after the prefix comes a `/`. If
/// that is not the case, this [`dioxus_history::History`] will replace the first character after the prefix
/// with one.
///
/// Application developers are responsible for not rendering the router if the prefix is not present
/// in the URL. Otherwise, if a router navigation is triggered, the prefix will be added.
pub struct WebHistory {
    do_scroll_restoration: bool,
    history: History,
    prefix: Option<String>,
    window: Window,
}

impl Default for WebHistory {
    fn default() -> Self {
        Self::new(None, true)
    }
}

impl WebHistory {
    /// Create a new [`WebHistory`].
    ///
    /// If `do_scroll_restoration` is [`true`], [`WebHistory`] will take control of the history
    /// state. It'll also set the browsers scroll restoration to `manual`.
    pub fn new(prefix: Option<String>, do_scroll_restoration: bool) -> Self {
        let myself = Self::new_inner(prefix, do_scroll_restoration);

        let current_route = dioxus_history::History::current_route(&myself);
        let current_route_str = current_route.to_string();
        let prefix_str = myself.prefix.as_deref().unwrap_or("");
        let current_url = format!("{prefix_str}{current_route_str}");
        let state = myself.create_state();
        let _ = replace_state_with_url(&myself.history, &state, Some(&current_url));

        myself
    }

    fn new_inner(prefix: Option<String>, do_scroll_restoration: bool) -> Self {
        let window = window().expect("access to `window`");
        let history = window.history().expect("`window` has access to `history`");

        if do_scroll_restoration {
            history
                .set_scroll_restoration(ScrollRestoration::Manual)
                .expect("`history` can set scroll restoration");
        }

        let prefix = prefix
            // If there isn't a base path, try to grab one from the CLI
            .or_else(dioxus_cli_config::web_base_path)
            // Normalize the prefix to start and end with no slashes
            .as_ref()
            .map(|prefix| prefix.trim_matches('/'))
            // If the prefix is empty, don't add it
            .filter(|prefix| !prefix.is_empty())
            // Otherwise, start with a slash
            .map(|prefix| format!("/{prefix}"));

        Self {
            do_scroll_restoration,
            history,
            prefix,
            window,
        }
    }

    fn scroll_pos(&self) -> ScrollPosition {
        if self.do_scroll_restoration {
            ScrollPosition::of_window(&self.window)
        } else {
            Default::default()
        }
    }

    fn create_state(&self) -> [f64; 2] {
        let scroll = self.scroll_pos();
        [scroll.x, scroll.y]
    }

    fn handle_nav(&self) {
        if self.do_scroll_restoration {
            self.window.scroll_to_with_x_and_y(0.0, 0.0)
        }
    }

    fn route_from_location(&self) -> String {
        let location = self.window.location();
        let path = location.pathname().unwrap_or_else(|_| "/".into())
            + &location.search().unwrap_or("".into())
            + &location.hash().unwrap_or("".into());
        let mut path = match self.prefix {
            None => &path,
            Some(ref prefix) => path.strip_prefix(prefix).unwrap_or(prefix),
        };
        // If the path is empty, parse the root route instead
        if path.is_empty() {
            path = "/"
        }
        path.to_string()
    }

    fn full_path(&self, state: &String) -> String {
        match &self.prefix {
            None => state.to_string(),
            Some(prefix) => format!("{prefix}{state}"),
        }
    }

    /// Leave the document for `url` when the History API would not take it.
    ///
    /// WebKit refuses history writes for a document that is over its state
    /// budget or whose origin it treats as sandboxed, and the router then
    /// renders the old route with nothing logged. A full navigation recovers
    /// the user. Only one is ever taken per document: a page that navigated
    /// itself during startup would otherwise reload forever.
    fn fallback_navigation(&self, url: &str) {
        thread_local! {
            static FALLBACK_TAKEN: Cell<bool> = const { Cell::new(false) };
        }
        if FALLBACK_TAKEN.with(|taken| taken.replace(true)) {
            tracing::error!(
                "history fallback already used by this document; not navigating to {url:?}"
            );
            return;
        }
        // Same effect as `History::external`, without needing that trait in
        // scope from this inherent impl.
        let _ = self.window.location().set_href(url);
    }
}

fn current_href(window: &Window) -> String {
    window.location().href().unwrap_or_default()
}

/// `url` resolved against the document, as the browser would write it.
fn resolve_href(window: &Window, url: &str) -> Option<String> {
    web_sys::Url::new_with_base(url, &current_href(window))
        .ok()
        .map(|resolved| resolved.href())
}

/// `pushState` returned normally, yet the address did not move.
///
/// WebKit drops a push without throwing when a Navigation API `navigate`
/// listener cancels it or the frame is detached. A push to the address the
/// document already has is the one honest way for the location to stay put,
/// so that case, and a target that does not resolve at all, are not drops.
fn push_was_silently_dropped(before: &str, after: &str, resolved_target: Option<&str>) -> bool {
    before == after && matches!(resolved_target, Some(target) if target != before)
}

impl dioxus_history::History for WebHistory {
    fn current_route(&self) -> String {
        self.route_from_location()
    }

    fn current_prefix(&self) -> Option<String> {
        self.prefix.clone()
    }

    fn go_back(&self) {
        let _ = self.history.back();
    }

    fn go_forward(&self) {
        let _ = self.history.forward();
    }

    fn push(&self, state: String) {
        if state == self.current_route() {
            // don't push the same state twice
            return;
        }

        let w = window().expect("access to `window`");
        let h = w.history().expect("`window` has access to `history`");

        // update the scroll position before pushing the new state
        update_scroll(&w, &h);

        let url = self.full_path(&state);
        let before = current_href(&w);
        match push_state_and_url(&self.history, &self.create_state(), url.clone()) {
            Ok(()) => {
                let after = current_href(&w);
                if push_was_silently_dropped(&before, &after, resolve_href(&w, &url).as_deref()) {
                    tracing::error!(
                        "history.pushState({url:?}) returned without moving the location from {before:?}; falling back to a full navigation"
                    );
                    self.fallback_navigation(&url);
                    return;
                }
                self.handle_nav();
            }
            Err(err) => {
                tracing::error!(
                    "history.pushState({url:?}) failed: {err:?}; falling back to a full navigation"
                );
                self.fallback_navigation(&url);
            }
        }
    }

    fn replace(&self, state: String) {
        let url = self.full_path(&state);
        match replace_state_with_url(&self.history, &self.create_state(), Some(&url)) {
            Ok(()) => self.handle_nav(),
            // A rejected replace leaves the address where it was, which the app
            // survives. Reloading here instead could turn a replace made during
            // startup into a reload loop, so this only reports.
            Err(err) => tracing::error!("history.replaceState({url:?}) failed: {err:?}"),
        }
    }

    fn external(&self, url: String) -> bool {
        self.window.location().set_href(&url).is_ok()
    }

    fn updater(&self, callback: std::sync::Arc<dyn Fn() + Send + Sync>) {
        let w = self.window.clone();
        let h = self.history.clone();
        let d = self.do_scroll_restoration;

        let function = Closure::wrap(Box::new(move |_| {
            (*callback)();
            if d {
                if let Some([x, y]) = get_current(&h) {
                    ScrollPosition { x, y }.scroll_to(w.clone())
                }
            }
        }) as Box<dyn FnMut(Event)>);
        self.window
            .add_event_listener_with_callback(
                "popstate",
                &function.into_js_value().unchecked_into(),
            )
            .unwrap();
    }
}

/// A [`dioxus_history::History`] provider that integrates with a browser via the [History API](https://developer.mozilla.org/en-US/docs/Web/API/History_API)
/// but uses the url fragment for the route. This allows serving as a single html file or on a single url path.
pub struct HashHistory {
    do_scroll_restoration: bool,
    history: History,
    pathname: String,
    window: Window,
}

impl Default for HashHistory {
    fn default() -> Self {
        Self::new(true)
    }
}

impl HashHistory {
    /// Create a new [`HashHistory`].
    ///
    /// If `do_scroll_restoration` is [`true`], [`HashHistory`] will take control of the history
    /// state. It'll also set the browsers scroll restoration to `manual`.
    pub fn new(do_scroll_restoration: bool) -> Self {
        let myself = Self::new_inner(do_scroll_restoration);

        let current_route = dioxus_history::History::current_route(&myself);
        let current_route_str = current_route.to_string();
        let pathname_str = &myself.pathname;
        let current_url = format!("{pathname_str}#{current_route_str}");
        let state = myself.create_state();
        let _ = replace_state_with_url(&myself.history, &state, Some(&current_url));

        myself
    }

    fn new_inner(do_scroll_restoration: bool) -> Self {
        let window = window().expect("access to `window`");
        let history = window.history().expect("`window` has access to `history`");
        let pathname = window.location().pathname().unwrap();

        if do_scroll_restoration {
            history
                .set_scroll_restoration(ScrollRestoration::Manual)
                .expect("`history` can set scroll restoration");
        }

        Self {
            do_scroll_restoration,
            history,
            pathname,
            window,
        }
    }

    fn scroll_pos(&self) -> ScrollPosition {
        if self.do_scroll_restoration {
            ScrollPosition::of_window(&self.window)
        } else {
            Default::default()
        }
    }

    fn create_state(&self) -> [f64; 2] {
        let scroll = self.scroll_pos();
        [scroll.x, scroll.y]
    }

    fn full_path(&self, state: &String) -> String {
        format!("{}#{state}", self.pathname)
    }

    fn handle_nav(&self) {
        if self.do_scroll_restoration {
            self.window.scroll_to_with_x_and_y(0.0, 0.0)
        }
    }
}

impl dioxus_history::History for HashHistory {
    fn current_route(&self) -> String {
        let location = self.window.location();

        let hash = location.hash().unwrap();
        if hash.is_empty() {
            // If the path is empty, parse the root route instead
            "/".to_owned()
        } else {
            hash.trim_start_matches("#").to_owned()
        }
    }

    fn current_prefix(&self) -> Option<String> {
        Some(format!("{}#", self.pathname))
    }

    fn go_back(&self) {
        let _ = self.history.back();
    }

    fn go_forward(&self) {
        let _ = self.history.forward();
    }

    fn push(&self, state: String) {
        if state == self.current_route() {
            // don't push the same state twice
            return;
        }

        let w = window().expect("access to `window`");
        let h = w.history().expect("`window` has access to `history`");

        // update the scroll position before pushing the new state
        update_scroll(&w, &h);

        if push_state_and_url(&self.history, &self.create_state(), self.full_path(&state)).is_ok() {
            self.handle_nav();
        }
    }

    fn replace(&self, state: String) {
        if replace_state_with_url(
            &self.history,
            &self.create_state(),
            Some(&self.full_path(&state)),
        )
        .is_ok()
        {
            self.handle_nav();
        }
    }

    fn external(&self, url: String) -> bool {
        self.window.location().set_href(&url).is_ok()
    }

    fn updater(&self, callback: std::sync::Arc<dyn Fn() + Send + Sync>) {
        let w = self.window.clone();
        let h = self.history.clone();
        let d = self.do_scroll_restoration;

        let function = Closure::wrap(Box::new(move |_| {
            (*callback)();
            if d {
                if let Some([x, y]) = get_current(&h) {
                    ScrollPosition { x, y }.scroll_to(w.clone())
                }
            }
        }) as Box<dyn FnMut(Event)>);
        self.window
            .add_event_listener_with_callback(
                "popstate",
                &function.into_js_value().unchecked_into(),
            )
            .unwrap();
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ScrollPosition {
    pub x: f64,
    pub y: f64,
}

impl ScrollPosition {
    pub(crate) fn of_window(window: &Window) -> Self {
        Self {
            x: window.scroll_x().unwrap_or_default(),
            y: window.scroll_y().unwrap_or_default(),
        }
    }

    pub(crate) fn scroll_to(&self, window: Window) {
        let Self { x, y } = *self;
        let f = Closure::wrap(
            Box::new(move || window.scroll_to_with_x_and_y(x, y)) as Box<dyn FnMut()>
        );
        web_sys::window()
            .expect("should be run in a context with a `Window` object (dioxus cannot be run from a web worker)")
            .request_animation_frame(&f.into_js_value().unchecked_into())
            .expect("should register `requestAnimationFrame` OK");
    }
}

pub(crate) fn replace_state_with_url(
    history: &History,
    value: &[f64; 2],
    url: Option<&str>,
) -> Result<(), JsValue> {
    let position = js_sys::Array::new();
    position.push(&JsValue::from(value[0]));
    position.push(&JsValue::from(value[1]));
    history.replace_state_with_url(&position, "", url)
}

pub(crate) fn push_state_and_url(
    history: &History,
    value: &[f64; 2],
    url: String,
) -> Result<(), JsValue> {
    let position = js_sys::Array::new();
    position.push(&JsValue::from(value[0]));
    position.push(&JsValue::from(value[1]));
    history.push_state_with_url(&position, "", Some(&url))
}

pub(crate) fn get_current(history: &History) -> Option<[f64; 2]> {
    use wasm_bindgen::JsCast;
    history.state().ok().and_then(|state| {
        let state = state.dyn_into::<js_sys::Array>().ok()?;
        let x = state.get(0).as_f64()?;
        let y = state.get(1).as_f64()?;
        Some([x, y])
    })
}

fn update_scroll(window: &Window, history: &History) {
    let scroll = ScrollPosition::of_window(window);
    let _ = replace_state_with_url(history, &[scroll.x, scroll.y], None);
}

#[cfg(test)]
mod tests {
    use super::push_was_silently_dropped;

    #[test]
    fn a_push_that_moved_the_address_is_fine() {
        assert!(!push_was_silently_dropped(
            "app:/home",
            "app:/cappers/",
            Some("app:/cappers/")
        ));
    }

    #[test]
    fn a_push_to_the_current_address_is_not_a_drop() {
        assert!(!push_was_silently_dropped(
            "app:/home",
            "app:/home",
            Some("app:/home")
        ));
    }

    #[test]
    fn an_address_that_did_not_move_is_a_drop() {
        assert!(push_was_silently_dropped(
            "app:/home",
            "app:/home",
            Some("app:/cappers/")
        ));
    }

    #[test]
    fn a_target_that_does_not_resolve_is_left_alone() {
        assert!(!push_was_silently_dropped("app:/home", "app:/home", None));
    }
}
