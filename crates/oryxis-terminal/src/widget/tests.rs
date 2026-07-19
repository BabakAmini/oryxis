    use super::*;

    fn view_and_state() -> (TerminalView<()>, TerminalWidgetState) {
        let term = TerminalState::new_no_pty(80, 24).unwrap();
        let view = TerminalView::new(Arc::new(Mutex::new(term)));
        (view, TerminalWidgetState::default())
    }

    fn bounds() -> Rectangle {
        Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 480.0))
    }

    /// SGR click tracking (mc, htop). Regression for the "must hold
    /// Shift to click the sidebar" report: a release whose press was
    /// never reported (it landed on a sibling widget, so the cursor is
    /// outside the canvas and no press is tracked) must NOT be consumed
    /// by the report path; capturing it starves sibling `button`s,
    /// which fire on release.
    #[test]
    fn untracked_release_is_not_reported() {
        use alacritty_terminal::term::TermMode;
        let (view, mut ws) = view_and_state();
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let ev = iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        // Cursor over the sidebar (outside the canvas), no tracked press.
        let cursor = mouse::Cursor::Available(Point::new(2000.0, 100.0));
        assert!(ws.report_button.is_none());
        let action = view.handle_mouse_report(&mut ws, &ev, bounds(), cursor, mode, 80, 24);
        assert!(action.is_none(), "sidebar release must stay local");
    }

    /// The canvas-originated press → drag off-canvas → release flow must
    /// still report the release (apps need the button-up to end a drag),
    /// falling back to the last reported cell.
    #[test]
    fn tracked_release_still_reports_after_leaving_canvas() {
        use alacritty_terminal::term::TermMode;
        let (view, mut ws) = view_and_state();
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;

        let press = iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let inside = mouse::Cursor::Available(Point::new(40.0, 40.0));
        let action = view.handle_mouse_report(&mut ws, &press, bounds(), inside, mode, 80, 24);
        assert!(action.is_some(), "on-canvas press must be reported");
        assert_eq!(ws.report_button, Some(ReportButton::Left));

        let release = iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        let outside = mouse::Cursor::Available(Point::new(2000.0, 100.0));
        let action = view.handle_mouse_report(&mut ws, &release, bounds(), outside, mode, 80, 24);
        assert!(action.is_some(), "release of a reported press must land");
        assert!(ws.report_button.is_none(), "press tracking cleared on release");
    }

    /// Pressing Shift AFTER a reported press must not swallow the
    /// release: `release_completes_tracked_press` lets it through the
    /// Shift bypass, so the app gets its button-up and `report_button`
    /// clears instead of sticking at `Some(Left)` (phantom held button,
    /// every later motion misread as a drag).
    #[test]
    fn shift_at_release_does_not_swallow_tracked_release() {
        use alacritty_terminal::term::TermMode;
        let (view, mut ws) = view_and_state();
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;

        let press = iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let inside = mouse::Cursor::Available(Point::new(40.0, 40.0));
        let action = view.handle_mouse_report(&mut ws, &press, bounds(), inside, mode, 80, 24);
        assert!(action.is_some(), "press without Shift must be reported");
        assert_eq!(ws.report_button, Some(ReportButton::Left));

        // Shift lands between press and release.
        ws.modifiers = iced::keyboard::Modifiers::SHIFT;
        let release = iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        assert!(
            TerminalView::<()>::release_completes_tracked_press(&ws, &release),
            "tracked release must pierce the Shift bypass"
        );
        let action = view.handle_mouse_report(&mut ws, &release, bounds(), inside, mode, 80, 24);
        assert!(action.is_some(), "release of a tracked press reports despite Shift");
        assert!(ws.report_button.is_none(), "press tracking cleared on release");
    }

    /// The Shift bypass must keep blocking NEW gestures: with no
    /// tracked press, neither a Shift+press nor its release qualifies
    /// as completing a tracked press, so local selection stays in
    /// charge for the whole gesture.
    #[test]
    fn shift_bypass_still_blocks_new_gestures() {
        let (_view, ws) = view_and_state();
        assert!(ws.report_button.is_none());
        let press = iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let release = iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        assert!(
            !TerminalView::<()>::release_completes_tracked_press(&ws, &press),
            "a press never qualifies"
        );
        assert!(
            !TerminalView::<()>::release_completes_tracked_press(&ws, &release),
            "a release with no tracked press never qualifies"
        );
    }

    /// `right_click_copy` is a Paste-scheme sub-option: a stale `true`
    /// under Menu / Extend (Settings hides the toggle there, so the
    /// user can't see or clear it) must not defer, i.e. suppress, the
    /// copy-on-select auto-copy.
    #[test]
    fn right_click_copy_only_defers_auto_copy_under_paste_scheme() {
        let (view, _) = view_and_state();
        let paste = view.with_right_click_copy(true).with_right_click_action(RightClickAction::Paste);
        assert!(paste.defers_copy_to_right_click(), "Paste scheme honours the deferral");

        let (view, _) = view_and_state();
        let menu = view.with_right_click_copy(true).with_right_click_action(RightClickAction::Menu);
        assert!(!menu.defers_copy_to_right_click(), "stale flag under Menu must not defer");

        let (view, _) = view_and_state();
        let extend = view.with_right_click_copy(true).with_right_click_action(RightClickAction::Extend);
        assert!(!extend.defers_copy_to_right_click(), "stale flag under Extend must not defer");

        let (view, _) = view_and_state();
        let off = view.with_right_click_action(RightClickAction::Paste);
        assert!(!off.defers_copy_to_right_click(), "flag off never defers");
    }
