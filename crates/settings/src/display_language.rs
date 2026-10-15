use crate::{self as settings, DisplayLanguage, RegisterSetting, Settings, SettingsContent};
use gpui::App;

#[derive(Clone, Copy, Debug, RegisterSetting)]
pub struct DisplayLanguageSettings(pub DisplayLanguage);

impl Settings for DisplayLanguageSettings {
    fn from_settings(content: &SettingsContent) -> Self {
        Self(content.display_language.unwrap_or_default())
    }
}

impl DisplayLanguageSettings {
    pub fn locale(self) -> i18n::Locale {
        match self.0 {
            DisplayLanguage::English => i18n::Locale::English,
            DisplayLanguage::SimplifiedChinese => i18n::Locale::SimplifiedChinese,
        }
    }

    pub fn restart_required(cx: &App) -> bool {
        cx.try_global::<i18n::Locale>()
            .is_some_and(|active| *active != Self::get_global(cx).locale())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SettingsStore;
    use gpui::UpdateGlobal;

    #[gpui::test]
    fn display_language_requires_restart_and_preserves_active_locale(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            crate::init(cx);
            assert_eq!(
                DisplayLanguageSettings::get_global(cx).0,
                DisplayLanguage::SimplifiedChinese
            );
            cx.set_global(DisplayLanguageSettings::get_global(cx).locale());
            assert!(!DisplayLanguageSettings::restart_required(cx));
            SettingsStore::update_global(cx, |store, cx| {
                store
                    .set_user_settings(r#"{"display_language":"en"}"#, cx)
                    .unwrap();
            });
            assert!(DisplayLanguageSettings::restart_required(cx));
            assert_eq!(i18n::locale(cx), i18n::Locale::SimplifiedChinese);
            assert_eq!(
                DisplayLanguageSettings::get_global(cx).locale(),
                i18n::Locale::English
            );
            SettingsStore::update_global(cx, |store, cx| {
                store
                    .set_user_settings(r#"{"display_language":"zh-CN"}"#, cx)
                    .unwrap();
            });
            assert!(!DisplayLanguageSettings::restart_required(cx));
        });
    }
}
