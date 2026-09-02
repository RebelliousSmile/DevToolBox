//! Pure native-material policy plus small, cfg-bounded OS adapters.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeProfile {
    MacVibrancy,
    WindowsMica,
    Opaque,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostPlatform {
    MacOS,
    Windows,
    Linux,
    Other,
}

#[derive(Clone, Copy, Debug)]
pub struct MaterialInputs {
    pub platform: HostPlatform,
    pub native_effects: bool,
    pub reduce_transparency: bool,
    pub system_support: bool,
}

pub fn decide(inputs: MaterialInputs) -> NativeProfile {
    if !inputs.native_effects || inputs.reduce_transparency || !inputs.system_support {
        return NativeProfile::Opaque;
    }
    match inputs.platform {
        HostPlatform::MacOS => NativeProfile::MacVibrancy,
        HostPlatform::Windows => NativeProfile::WindowsMica,
        HostPlatform::Linux | HostPlatform::Other => NativeProfile::Opaque,
    }
}

pub fn current_inputs(native_effects: bool) -> MaterialInputs {
    let reduce_transparency = std::env::var("DEVTOOLBOX_REDUCE_TRANSPARENCY")
        .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE"));
    MaterialInputs {
        platform: if cfg!(target_os = "macos") {
            HostPlatform::MacOS
        } else if cfg!(target_os = "windows") {
            HostPlatform::Windows
        } else if cfg!(target_os = "linux") {
            HostPlatform::Linux
        } else {
            HostPlatform::Other
        },
        native_effects,
        reduce_transparency,
        system_support: cfg!(any(target_os = "macos", target_os = "windows")),
    }
}

pub fn configure_viewport(builder: eframe::egui::ViewportBuilder) -> eframe::egui::ViewportBuilder {
    #[cfg(target_os = "macos")]
    {
        return builder
            .with_transparent(true)
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_titlebar_buttons_shown(true);
    }
    #[cfg(target_os = "windows")]
    {
        return builder.with_transparent(true);
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    builder
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn apply(
    window: &impl raw_window_handle::HasWindowHandle,
    previous: NativeProfile,
    desired: NativeProfile,
    dark: bool,
) -> Result<NativeProfile, String> {
    #[cfg(target_os = "macos")]
    let _ = dark;
    if previous == desired {
        return Ok(desired);
    }
    #[cfg(target_os = "macos")]
    {
        if previous == NativeProfile::MacVibrancy {
            let _ = window_vibrancy::clear_vibrancy(window);
        }
        if desired == NativeProfile::MacVibrancy {
            window_vibrancy::apply_vibrancy(
                window,
                window_vibrancy::NSVisualEffectMaterial::Sidebar,
                Some(window_vibrancy::NSVisualEffectState::Active),
                None,
            )
            .map_err(|error| error.to_string())?;
        }
    }
    #[cfg(target_os = "windows")]
    {
        if previous == NativeProfile::WindowsMica {
            let _ = window_vibrancy::clear_mica(window);
        }
        if desired == NativeProfile::WindowsMica {
            window_vibrancy::apply_mica(window, Some(dark)).map_err(|error| error.to_string())?;
        }
    }
    Ok(desired)
}

#[cfg(test)]
pub fn fallback_after_error(
    desired: NativeProfile,
    result: Result<(), impl std::fmt::Display>,
) -> (NativeProfile, Option<String>) {
    match result {
        Ok(()) => (desired, None),
        Err(error) => (NativeProfile::Opaque, Some(error.to_string())),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn apply<T>(
    _window: &T,
    _previous: NativeProfile,
    desired: NativeProfile,
    _dark: bool,
) -> Result<NativeProfile, String> {
    Ok(desired)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(platform: HostPlatform) -> MaterialInputs {
        MaterialInputs {
            platform,
            native_effects: true,
            reduce_transparency: false,
            system_support: true,
        }
    }

    #[test]
    fn every_combination_selects_exactly_one_profile() {
        assert_eq!(
            decide(inputs(HostPlatform::MacOS)),
            NativeProfile::MacVibrancy
        );
        assert_eq!(
            decide(inputs(HostPlatform::Windows)),
            NativeProfile::WindowsMica
        );
        assert_eq!(decide(inputs(HostPlatform::Linux)), NativeProfile::Opaque);
        assert_eq!(decide(inputs(HostPlatform::Other)), NativeProfile::Opaque);
    }

    #[test]
    fn preference_accessibility_and_missing_support_force_opaque() {
        let mut value = inputs(HostPlatform::MacOS);
        value.native_effects = false;
        assert_eq!(decide(value), NativeProfile::Opaque);
        value.native_effects = true;
        value.reduce_transparency = true;
        assert_eq!(decide(value), NativeProfile::Opaque);
        value.reduce_transparency = false;
        value.system_support = false;
        assert_eq!(decide(value), NativeProfile::Opaque);
    }

    #[test]
    fn accessibility_changes_are_recomputed_without_sticky_state() {
        let mut value = inputs(HostPlatform::Windows);
        assert_eq!(decide(value), NativeProfile::WindowsMica);
        value.reduce_transparency = true;
        assert_eq!(decide(value), NativeProfile::Opaque);
    }

    #[test]
    fn native_api_errors_produce_an_opaque_profile_and_one_diagnostic() {
        let (profile, diagnostic) =
            fallback_after_error(NativeProfile::MacVibrancy, Err("permission refused"));
        assert_eq!(profile, NativeProfile::Opaque);
        assert_eq!(diagnostic.as_deref(), Some("permission refused"));
    }
}
