//! Vault setup / unlock / error screens.

use iced::border::Radius;
use iced::widget::{button, column, container, svg, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding};

use crate::app::{Message, Oryxis};
use crate::theme::{mix, OryxisColors};
use crate::views::chrome::window_chrome_bar;
use crate::widgets::{accent_gradient, password_input_with_eye, styled_button};

/// Wrap a vault screen body with the top window chrome so the user can still
/// drag / minimize / maximize / close before unlocking the vault. Also adds
/// the edge-resize border so the lock screen is as resizable as the main app.
pub(crate) fn with_chrome<'a>(body: Element<'a, Message>, maximized: bool) -> Element<'a, Message> {
    // 1 px hairline between the chrome bar and the screen body, matches the
    // separator that sits below the tab bar on the main view.
    let h_separator = iced::widget::container(iced::widget::Space::new().height(1))
        .width(Length::Fill)
        .style(|_| iced::widget::container::Style {
            background: Some(iced::Background::Color(OryxisColors::t().border)),
            ..Default::default()
        });
    let content: Element<'a, Message> =
        iced::widget::column![window_chrome_bar(), h_separator, body]
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
    let overlay = if maximized { None } else { Some(crate::views::layout::resize_border()) };
    crate::views::layout::wrap_with_resize(content, overlay)
}

impl Oryxis {
    /// Master-password field. Wraps the shared `password_input_with_eye`
    /// helper with the vault's wider 300 px container and hero-sized
    /// inner padding.
    fn vault_master_password_field<'a>(
        &'a self,
        placeholder: &'a str,
        on_submit: Message,
    ) -> Element<'a, Message> {
        container(password_input_with_eye(
            placeholder,
            &self.vault_ui.password_input,
            Message::VaultPasswordChanged,
            Some(on_submit),
            self.vault_ui.password_visible,
            Message::VaultTogglePasswordVisibility,
            12.0,
        ))
        .width(300)
        .into()
    }

    // The first-run setup screen used to live here as `view_vault_setup`.
    // It is now the final slide of the onboarding carousel
    // (`views/onboarding.rs`), rendered off `VaultState::NeedSetup`.

    pub(crate) fn view_vault_unlock(&self) -> Element<'_, Message> {
        let logo = svg(self.logo_handle.clone())
            .width(64)
            .height(64);
        let title = text("Oryxis").size(28).color(OryxisColors::t().accent);
        let subtitle = text(crate::i18n::t("enter_password"))
            .size(14)
            .color(OryxisColors::t().text_secondary);

        let input = self.vault_master_password_field(
            crate::i18n::t("master_password_placeholder"),
            Message::VaultUnlock,
        );

        let btn = styled_button(crate::i18n::t("unlock"), Message::VaultUnlock, OryxisColors::t().accent);

        let error = if let Some(err) = &self.vault_ui.error {
            Element::from(text(err.clone()).size(13).color(OryxisColors::t().error))
        } else {
            Space::new().height(0).into()
        };

        let destroy_section: Element<'_, Message> = if self.vault_ui.destroy_confirm {
            column![
                text(crate::i18n::t("vault_destroy_confirm")).size(12).color(OryxisColors::t().error),
                Space::new().height(6),
                styled_button(crate::i18n::t("destroy_vault"), Message::VaultDestroy, OryxisColors::t().error),
            ].align_x(iced::Alignment::Center).into()
        } else {
            button(
                text(crate::i18n::t("forgot_password")).size(12).color(OryxisColors::t().text_muted),
            )
            .on_press(Message::VaultDestroyConfirm)
            .padding(Padding { top: 6.0, right: 12.0, bottom: 6.0, left: 12.0 })
            .style(|_, _| button::Style::default())
            .into()
        };

        // The unlock form sits on a gradient card centered on an
        // accent-washed page, sharing the onboarding carousel's design
        // language (see `views/onboarding.rs`): same card chrome (radius 18,
        // 1px border, soft drop shadow) and the same two-layer diagonal
        // accent gradient (`widgets::accent_gradient`).
        let card_inner = column![logo, Space::new().height(16), title, Space::new().height(8), subtitle, Space::new().height(24), input, Space::new().height(12), btn, Space::new().height(8), error, Space::new().height(16), destroy_section]
            .width(Length::Fill)
            .align_x(iced::Alignment::Center);

        let card = container(card_inner)
            .padding(Padding { top: 48.0, right: 48.0, bottom: 40.0, left: 48.0 })
            .width(Length::Fixed(460.0))
            .style(|_| {
                let base = OryxisColors::t().bg_primary;
                let accent = OryxisColors::t().accent;
                container::Style {
                    background: Some(accent_gradient(mix(base, accent, 0.12), base)),
                    border: Border {
                        radius: Radius::from(18.0),
                        color: OryxisColors::t().border,
                        width: 1.0,
                    },
                    shadow: iced::Shadow {
                        color: Color { a: 0.32, ..Color::BLACK },
                        offset: iced::Vector::new(0.0, 12.0),
                        blur_radius: 40.0,
                    },
                    ..Default::default()
                }
            });

        let body: Element<'_, Message> = container(card)
            .center(Length::Fill)
            .style(|_| {
                let base = OryxisColors::t().bg_sidebar;
                let accent = OryxisColors::t().accent;
                container::Style {
                    background: Some(accent_gradient(mix(base, accent, 0.22), base)),
                    ..Default::default()
                }
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        with_chrome(body, self.window_maximized)
    }

    pub(crate) fn view_vault_error(&self, msg: &str) -> Element<'_, Message> {
        let msg = msg.to_string();
        let body: Element<'_, Message> = container(
            text(msg).size(16).color(OryxisColors::t().error),
        )
        .center(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(OryxisColors::t().bg_primary)),
            ..Default::default()
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
        with_chrome(body, self.window_maximized)
    }

    // -- Main layout --
}
