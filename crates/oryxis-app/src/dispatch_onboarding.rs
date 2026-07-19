//! `Oryxis::handle_onboarding`: dispatch arms for the welcome / onboarding
//! carousel, split out of dispatch.rs. Returns `Err(message)` for anything
//! it doesn't claim so the try_handler! chain falls through.
#![allow(clippy::result_large_err)]

use iced::Task;

use crate::app::{Message, OnboardingMessage, Oryxis};

/// Last slide index. The carousel has five slides (0..=4); the final one
/// carries the master-password setup. Kept here as the single source of
/// truth for navigation clamping and the "Skip" jump.
pub(crate) const ONBOARDING_LAST_SLIDE: usize = 4;

impl Oryxis {
    pub(crate) fn handle_onboarding(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        let Message::Onboarding(m) = message else {
            return Err(message);
        };
        match m {
            OnboardingMessage::Next => {
                if self.onboarding_slide < ONBOARDING_LAST_SLIDE {
                    self.onboarding_slide += 1;
                }
            }
            OnboardingMessage::Back => {
                if self.onboarding_slide > 0 {
                    self.onboarding_slide -= 1;
                }
            }
            OnboardingMessage::SkipToEnd => {
                self.onboarding_slide = ONBOARDING_LAST_SLIDE;
            }
        }
        Ok(Task::none())
    }
}
