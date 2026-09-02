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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RendererBackend {
    Dx12,
    Metal,
    Other,
    Unavailable,
}

#[derive(Clone, Copy, Debug)]
pub struct MaterialInputs {
    pub platform: HostPlatform,
    pub native_effects: bool,
    pub reduce_transparency: bool,
    pub system_support: bool,
    pub renderer_support: bool,
}

pub fn decide(inputs: MaterialInputs) -> NativeProfile {
    if !inputs.native_effects
        || inputs.reduce_transparency
        || !inputs.system_support
        || !inputs.renderer_support
    {
        return NativeProfile::Opaque;
    }
    match inputs.platform {
        HostPlatform::MacOS => NativeProfile::MacVibrancy,
        HostPlatform::Windows => NativeProfile::WindowsMica,
        HostPlatform::Linux | HostPlatform::Other => NativeProfile::Opaque,
    }
}

pub fn current_inputs(native_effects: bool, renderer_support: bool) -> MaterialInputs {
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
        renderer_support,
    }
}

pub fn renderer_supports_material(platform: HostPlatform, backend: RendererBackend) -> bool {
    matches!(
        (platform, backend),
        (HostPlatform::Windows, RendererBackend::Dx12)
            | (HostPlatform::MacOS, RendererBackend::Metal)
    )
}

pub fn current_renderer_support(cc: &eframe::CreationContext<'_>) -> bool {
    // eframe 0.35 does not expose the surface `CompositeAlphaMode` selected
    // by its wgpu painter. Backend identity alone is insufficient: the QA
    // machine exposes DX12 yet reports no transparent composite mode, which
    // turns a maximized Mica window fully invisible. Until that capability
    // can be observed, Windows must take the safe opaque branch.
    let platform = current_inputs(true, true).platform;
    let backend = cc
        .wgpu_render_state
        .as_ref()
        .map(|state| match state.adapter.get_info().backend {
            eframe::wgpu::Backend::Dx12 => RendererBackend::Dx12,
            eframe::wgpu::Backend::Metal => RendererBackend::Metal,
            _ => RendererBackend::Other,
        })
        .unwrap_or(RendererBackend::Unavailable);
    let backend_support = renderer_supports_material(platform, backend);
    #[cfg(windows)]
    {
        log::info!(
            "native material renderer={backend:?} backend_support={backend_support}; alpha surface support is not observable, using opaque Windows fallback"
        );
        false
    }
    #[cfg(not(windows))]
    {
        backend_support
    }
}

fn backend_priority(backend: eframe::wgpu::Backend) -> u8 {
    if backend == eframe::wgpu::Backend::Dx12 {
        0
    } else {
        1
    }
}

fn device_priority(device: eframe::wgpu::DeviceType) -> u8 {
    match device {
        eframe::wgpu::DeviceType::DiscreteGpu => 0,
        eframe::wgpu::DeviceType::IntegratedGpu => 1,
        eframe::wgpu::DeviceType::VirtualGpu => 2,
        eframe::wgpu::DeviceType::Cpu => 3,
        eframe::wgpu::DeviceType::Other => 4,
    }
}

pub fn configure_renderer(options: &mut eframe::NativeOptions) {
    #[cfg(windows)]
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        setup.native_adapter_selector = Some(std::sync::Arc::new(|adapters, surface| {
            adapters
                .iter()
                .filter(|adapter| {
                    surface.is_none_or(|surface| adapter.is_surface_supported(surface))
                })
                .min_by_key(|adapter| {
                    let info = adapter.get_info();
                    (
                        backend_priority(info.backend),
                        device_priority(info.device_type),
                    )
                })
                .cloned()
                .ok_or_else(|| "aucun adaptateur wgpu compatible avec la surface".to_string())
        }));
    }

    #[cfg(not(windows))]
    let _ = options;
}

pub fn configure_viewport(builder: eframe::egui::ViewportBuilder) -> eframe::egui::ViewportBuilder {
    #[cfg(target_os = "macos")]
    {
        builder
            .with_transparent(true)
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_titlebar_buttons_shown(true)
    }
    #[cfg(target_os = "windows")]
    {
        // See `current_renderer_support`: requesting an alpha surface when
        // wgpu cannot expose/guarantee one can make the entire window vanish.
        builder.with_transparent(false)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        builder
    }
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
            if let Err(error) = window_vibrancy::apply_mica(window, Some(dark)) {
                let _ = window_vibrancy::clear_mica(window);
                return Err(error.to_string());
            }
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
            renderer_support: true,
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
        value.system_support = true;
        value.renderer_support = false;
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

    #[test]
    fn native_material_requires_the_platform_renderer_pair() {
        assert!(renderer_supports_material(
            HostPlatform::Windows,
            RendererBackend::Dx12
        ));
        assert!(!renderer_supports_material(
            HostPlatform::Windows,
            RendererBackend::Other
        ));
        assert!(renderer_supports_material(
            HostPlatform::MacOS,
            RendererBackend::Metal
        ));
        assert!(!renderer_supports_material(
            HostPlatform::Linux,
            RendererBackend::Dx12
        ));
    }

    #[test]
    fn dx12_is_preferred_without_overriding_device_quality() {
        assert!(
            backend_priority(eframe::wgpu::Backend::Dx12)
                < backend_priority(eframe::wgpu::Backend::Vulkan)
        );
        assert!(
            device_priority(eframe::wgpu::DeviceType::DiscreteGpu)
                < device_priority(eframe::wgpu::DeviceType::Cpu)
        );
    }
}
