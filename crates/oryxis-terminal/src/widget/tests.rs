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

    /// Build a view over a terminal with `lines` rows of scrollback, so
    /// there is somewhere to scroll to.
    fn scrolled_view(lines: usize) -> (TerminalView<()>, TerminalWidgetState) {
        let mut term = TerminalState::new_no_pty(80, 24).unwrap();
        for _ in 0..lines {
            term.process(b"line\r\n");
        }
        (
            TerminalView::new(Arc::new(Mutex::new(term))),
            TerminalWidgetState::default(),
        )
    }

    /// A scrolled-back terminal driven only by `ScrollDelta::Pixels`
    /// deltas smaller than one cell (Windows precision touchpads and
    /// high-res wheels deliver a few pixels per notch): the pre-#91
    /// handler floored each `y / cell_height` to zero, so scrollback
    /// never moved and the transcript viewer (no output to snap it back)
    /// was frozen. The residual accumulator now carries the sub-cell
    /// remainder across events and emits a whole line once the pixels
    /// cross a cell.
    #[test]
    fn subcell_pixel_wheel_accumulates_into_scroll() {
        let (view, mut ws) = scrolled_view(200);
        // Cursor over the canvas; start at the live edge (offset 0).
        let cursor = mouse::Cursor::Available(Point::new(40.0, 40.0));
        assert_eq!(ws.scroll_offset.get(), 0);

        // cell_height defaults to 14.0 * 1.15 = 16.1, so a 10px notch is
        // sub-cell: one alone must not move (correct), but the second
        // crosses a cell boundary and advances exactly one line, where
        // the old truncation stayed pinned at zero forever.
        let notch = iced::Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Pixels { x: 0.0, y: 10.0 },
        });
        let action = view.on_event(&mut ws, &notch, bounds(), cursor);
        assert!(action.is_some(), "the canvas consumes the wheel event");
        assert_eq!(ws.scroll_offset.get(), 0, "one sub-cell notch must not move");

        view.on_event(&mut ws, &notch, bounds(), cursor);
        assert_eq!(ws.scroll_offset.get(), 1, "two sub-cell notches cross a cell");

        // Five more keep it climbing, proving the residual never stalls.
        for _ in 0..5 {
            view.on_event(&mut ws, &notch, bounds(), cursor);
        }
        assert!(
            ws.scroll_offset.get() >= 4,
            "sub-cell pixel wheel keeps advancing, got {}",
            ws.scroll_offset.get()
        );
    }

    /// A `ScrollDelta::Lines` notch still moves whole lines and clears
    /// any carried pixel residual, so switching devices (touchpad →
    /// discrete wheel) can't leave a stale sub-cell fraction fighting the
    /// next notch.
    #[test]
    fn line_wheel_moves_and_clears_pixel_residual() {
        let (view, mut ws) = scrolled_view(200);
        let cursor = mouse::Cursor::Available(Point::new(40.0, 40.0));

        // Leave a sub-cell residual behind from a pixel notch.
        let px = iced::Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Pixels { x: 0.0, y: 10.0 },
        });
        view.on_event(&mut ws, &px, bounds(), cursor);
        assert_ne!(ws.scroll_px_residual.get(), 0.0, "pixel notch left a residual");

        // A line notch scrolls 3 lines (y * 3) and wipes the residual.
        let ln = iced::Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 },
        });
        view.on_event(&mut ws, &ln, bounds(), cursor);
        assert_eq!(ws.scroll_offset.get(), 3, "one line notch scrolls 3 lines");
        assert_eq!(ws.scroll_px_residual.get(), 0.0, "line notch clears the residual");
    }
