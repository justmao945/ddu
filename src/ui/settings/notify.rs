//! Notification settings: whether an agent's "your turn" marker
//! (`BEL` / `OSC 9` / `OSC 777`, see `terminal::attention`) raises a
//! desktop notification while ddu is not showing that terminal.

use super::update_config;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::setting::SettingItem;
use gpui_kit::component::switch::Switch;

pub(super) fn notify_item() -> SettingItem {
    super::item(
        "Desktop notifications",
        "Notify when an agent finishes a turn or waits for input while that \
         terminal is not in front of you.",
        |options, _, cx| {
            let on = cx.global::<crate::config::Config>().notify_on_attention();
            Switch::new("notify-on-attention")
                .checked(on)
                .disabled(options.is_disabled())
                .with_size(options.size())
                .on_click(|checked, _, cx| {
                    update_config(|config, _| config.notify_on_attention = Some(*checked), cx)
                })
        },
    )
}
