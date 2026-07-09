//! Adapter selection: hardware Vulkan, lavapipe, or a clear refusal.
//!
//! One code path — wgpu → Vulkan → (hardware driver | lavapipe) — so the choice
//! here is not *which renderer* but *which adapter the one renderer runs on*
//! (`VISION.md` §5.3, §7).
//!
//! `force_fallback_adapter` alone is not enough to answer either question.
//! Asking for a non-fallback adapter on a GPU-less host still returns lavapipe,
//! because it is the only Vulkan adapter present. So selection asks for what it
//! wants and then *checks what it got* against [`AdapterKind`], which is the
//! only way `--adapter gpu` can fail fast instead of silently rendering at 8 fps.

use std::fmt;
use std::str::FromStr;

use crate::{Error, Result};

/// Which adapter the user asked for (`--adapter auto|gpu|software`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AdapterChoice {
    /// Prefer hardware; fall back to software with a warning.
    #[default]
    Auto,
    /// Hardware only. A GPU-less host is an error, not a slow render.
    Gpu,
    /// Software only (lavapipe). What golden-frame tests and headless boxes use.
    Software,
}

/// What the selected adapter turned out to be.
///
/// Derived from the adapter's own `DeviceType`, never from what we requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterKind {
    /// A real GPU: discrete, integrated, or virtualized.
    Hardware,
    /// A CPU implementation of Vulkan, in practice Mesa's lavapipe.
    Software,
}

/// The adapter selection succeeded, and this is what it settled on.
#[derive(Debug)]
pub struct Selection {
    adapter: wgpu::Adapter,
    kind: AdapterKind,
    requested: AdapterChoice,
}

impl AdapterChoice {
    /// The spellings accepted on the command line and in config files.
    const NAMES: [(&'static str, AdapterChoice); 3] = [
        ("auto", AdapterChoice::Auto),
        ("gpu", AdapterChoice::Gpu),
        ("software", AdapterChoice::Software),
    ];

    /// Whether an adapter of this kind satisfies what the user asked for.
    ///
    /// The whole policy in one function: `auto` takes anything, `gpu` refuses
    /// software, `software` refuses hardware — the last so a reproducibility
    /// test cannot quietly run on the developer's GPU.
    pub fn accepts(self, kind: AdapterKind) -> bool {
        match self {
            AdapterChoice::Auto => true,
            AdapterChoice::Gpu => kind == AdapterKind::Hardware,
            AdapterChoice::Software => kind == AdapterKind::Software,
        }
    }

    /// Whether wgpu should be told to hand back its fallback adapter.
    fn force_fallback_adapter(self) -> bool {
        self == AdapterChoice::Software
    }

    /// How hard to push wgpu toward the fastest adapter present.
    fn power_preference(self) -> wgpu::PowerPreference {
        match self {
            AdapterChoice::Auto | AdapterChoice::Gpu => wgpu::PowerPreference::HighPerformance,
            AdapterChoice::Software => wgpu::PowerPreference::None,
        }
    }
}

impl AdapterKind {
    /// Classify an adapter by what it says it is.
    fn from_device_type(device_type: wgpu::DeviceType) -> Self {
        match device_type {
            wgpu::DeviceType::Cpu => AdapterKind::Software,
            wgpu::DeviceType::DiscreteGpu
            | wgpu::DeviceType::IntegratedGpu
            | wgpu::DeviceType::VirtualGpu
            | wgpu::DeviceType::Other => AdapterKind::Hardware,
        }
    }
}

impl Selection {
    /// The adapter to open a device on.
    pub fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    /// What the adapter actually is.
    pub fn kind(&self) -> AdapterKind {
        self.kind
    }

    /// Whether `auto` had to settle for software rendering.
    ///
    /// `avz-cli` turns this into the one actionable warning; core never prints.
    pub fn fell_back_to_software(&self) -> bool {
        fell_back_to_software(self.requested, self.kind)
    }
}

/// Whether the user should be warned that this render will be slow.
///
/// Only `auto` warrants a warning. Someone who passed `--adapter software` asked
/// for lavapipe and must not be told about it every render (`VISION.md` §3).
fn fell_back_to_software(requested: AdapterChoice, kind: AdapterKind) -> bool {
    requested == AdapterChoice::Auto && kind == AdapterKind::Software
}

impl FromStr for AdapterChoice {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::NAMES
            .iter()
            .find(|(name, _)| *name == s)
            .map(|(_, choice)| *choice)
            .ok_or_else(|| {
                let accepted = Self::NAMES
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>()
                    .join("|");
                Error::Config(format!("unknown adapter `{s}`; expected one of {accepted}"))
            })
    }
}

impl fmt::Display for AdapterChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = Self::NAMES
            .iter()
            .find(|(_, choice)| choice == self)
            .map(|(name, _)| *name)
            .expect("every choice is named");
        f.write_str(name)
    }
}

impl fmt::Display for AdapterKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AdapterKind::Hardware => "hardware",
            AdapterKind::Software => "software",
        })
    }
}

/// Fedora is the primary target, so its install line leads (`VISION.md` §7).
const LAVAPIPE_HINT: &str = "avz renders through Vulkan; \
     install Mesa's software Vulkan driver with `sudo dnf install mesa-vulkan-drivers` on Fedora, \
     or your distribution's equivalent, and verify it with `vulkaninfo --summary`";

/// Pick the adapter `choice` asks for.
///
/// `auto` requests a hardware adapter and, if none exists, retries with wgpu's
/// fallback adapter — reporting the fall back through
/// [`Selection::fell_back_to_software`] rather than printing anything.
///
/// # Errors
///
/// [`Error::Render`] when no adapter satisfies the choice: `gpu` on a GPU-less
/// host, or `software` where lavapipe is not installed. Both messages say what
/// to do next.
pub fn select(instance: &wgpu::Instance, choice: AdapterChoice) -> Result<Selection> {
    for attempt in attempts(choice) {
        if let Some(selection) = request(instance, *attempt) {
            return Ok(Selection {
                requested: choice,
                ..selection
            });
        }
    }

    Err(no_adapter(choice))
}

/// The requests to try, in order, for a given choice.
///
/// `auto` is the only choice with a second attempt, and it asks for hardware
/// first. Forcing wgpu's fallback adapter up front would skip a perfectly good
/// GPU; accepting whatever the first request returns would report a hardware
/// render on a host that quietly handed back lavapipe.
fn attempts(choice: AdapterChoice) -> &'static [AdapterChoice] {
    match choice {
        AdapterChoice::Auto => &[AdapterChoice::Gpu, AdapterChoice::Software],
        AdapterChoice::Gpu => &[AdapterChoice::Gpu],
        AdapterChoice::Software => &[AdapterChoice::Software],
    }
}

/// One `request_adapter` round trip, kept only if the adapter it returns is one
/// `choice` accepts.
fn request(instance: &wgpu::Instance, choice: AdapterChoice) -> Option<Selection> {
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: choice.power_preference(),
        force_fallback_adapter: choice.force_fallback_adapter(),
        compatible_surface: None,
        ..Default::default()
    }))
    .ok()?;

    let info = adapter.get_info();
    let kind = AdapterKind::from_device_type(info.device_type);
    if !choice.accepts(kind) {
        tracing::debug!(
            requested = %choice,
            got = %kind,
            adapter = %info.name,
            "discarding an adapter the request does not accept"
        );
        return None;
    }

    tracing::debug!(
        requested = %choice,
        kind = %kind,
        adapter = %info.name,
        backend = %info.backend,
        driver = %info.driver_info,
        "selected adapter"
    );

    Some(Selection {
        adapter,
        kind,
        requested: choice,
    })
}

/// Why nothing could be selected, and what the user can do about it.
fn no_adapter(choice: AdapterChoice) -> Error {
    Error::Render(match choice {
        AdapterChoice::Gpu => "no hardware GPU adapter found, but `--adapter gpu` demands one; \
             pass `--adapter software` to render on lavapipe instead (much slower), \
             or `--adapter auto` to let avz decide"
            .to_owned(),
        AdapterChoice::Auto | AdapterChoice::Software => {
            format!("no Vulkan adapter found, not even a software one. {LAVAPIPE_HINT}")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_choice_parses_the_documented_spellings() {
        assert_eq!(
            "auto".parse::<AdapterChoice>().expect("auto"),
            AdapterChoice::Auto
        );
        assert_eq!(
            "gpu".parse::<AdapterChoice>().expect("gpu"),
            AdapterChoice::Gpu
        );
        assert_eq!(
            "software".parse::<AdapterChoice>().expect("software"),
            AdapterChoice::Software
        );
    }

    #[test]
    fn the_default_adapter_choice_is_auto() {
        assert_eq!(AdapterChoice::default(), AdapterChoice::Auto);
    }

    #[test]
    fn every_adapter_choice_displays_as_the_name_it_parses_from() {
        for choice in [
            AdapterChoice::Auto,
            AdapterChoice::Gpu,
            AdapterChoice::Software,
        ] {
            let rendered = choice.to_string();
            assert_eq!(
                rendered.parse::<AdapterChoice>().expect("round trips"),
                choice
            );
        }
    }

    #[test]
    fn an_unknown_adapter_name_is_a_config_error_listing_the_options() {
        let err = "lavapipe"
            .parse::<AdapterChoice>()
            .expect_err("`lavapipe` is the driver, not the flag value");

        assert!(matches!(err, Error::Config(_)), "got {err:?}");
        let msg = err.to_string();
        assert!(
            msg.contains("lavapipe"),
            "message must quote the input: {msg}"
        );
        assert!(
            msg.contains("auto|gpu|software"),
            "message must list what is accepted: {msg}"
        );
    }

    /// The invariant that makes `--adapter gpu` fail fast: wgpu happily returns
    /// lavapipe for a non-fallback request on a GPU-less host, so the returned
    /// adapter's kind — not the request — decides.
    #[test]
    fn gpu_refuses_a_software_adapter_and_software_refuses_a_hardware_one() {
        assert!(AdapterChoice::Gpu.accepts(AdapterKind::Hardware));
        assert!(!AdapterChoice::Gpu.accepts(AdapterKind::Software));

        assert!(AdapterChoice::Software.accepts(AdapterKind::Software));
        assert!(
            !AdapterChoice::Software.accepts(AdapterKind::Hardware),
            "a golden-frame test must never silently run on a GPU"
        );
    }

    #[test]
    fn auto_accepts_whatever_vulkan_offers() {
        assert!(AdapterChoice::Auto.accepts(AdapterKind::Hardware));
        assert!(AdapterChoice::Auto.accepts(AdapterKind::Software));
    }

    #[test]
    fn only_a_cpu_device_type_counts_as_software_rendering() {
        assert_eq!(
            AdapterKind::from_device_type(wgpu::DeviceType::Cpu),
            AdapterKind::Software
        );
        for device_type in [
            wgpu::DeviceType::DiscreteGpu,
            wgpu::DeviceType::IntegratedGpu,
            wgpu::DeviceType::VirtualGpu,
            wgpu::DeviceType::Other,
        ] {
            assert_eq!(
                AdapterKind::from_device_type(device_type),
                AdapterKind::Hardware,
                "{device_type:?} is not lavapipe"
            );
        }
    }

    /// Only `software` forces wgpu's fallback adapter. `auto` asks for hardware
    /// first and retries; forcing it up front would skip the GPU entirely.
    #[test]
    fn only_the_software_choice_forces_the_fallback_adapter() {
        assert!(!AdapterChoice::Auto.force_fallback_adapter());
        assert!(!AdapterChoice::Gpu.force_fallback_adapter());
        assert!(AdapterChoice::Software.force_fallback_adapter());
    }

    #[test]
    fn auto_tries_hardware_before_falling_back_to_software() {
        assert_eq!(
            attempts(AdapterChoice::Auto),
            [AdapterChoice::Gpu, AdapterChoice::Software]
        );
    }

    /// An explicit choice never silently becomes the other one.
    #[test]
    fn an_explicit_adapter_choice_gets_exactly_one_attempt() {
        assert_eq!(attempts(AdapterChoice::Gpu), [AdapterChoice::Gpu]);
        assert_eq!(attempts(AdapterChoice::Software), [AdapterChoice::Software]);
    }

    /// The warning fires when `auto` settles for lavapipe, and never when the
    /// user asked for lavapipe by name.
    #[test]
    fn only_an_auto_render_that_lands_on_software_is_worth_warning_about() {
        assert!(fell_back_to_software(
            AdapterChoice::Auto,
            AdapterKind::Software
        ));

        assert!(!fell_back_to_software(
            AdapterChoice::Auto,
            AdapterKind::Hardware
        ));
        assert!(
            !fell_back_to_software(AdapterChoice::Software, AdapterKind::Software),
            "`--adapter software` silences the warning"
        );
        assert!(!fell_back_to_software(
            AdapterChoice::Gpu,
            AdapterKind::Hardware
        ));
    }

    #[test]
    fn asking_for_gpu_and_finding_none_says_how_to_render_anyway() {
        let err = no_adapter(AdapterChoice::Gpu);

        assert!(matches!(err, Error::Render(_)), "got {err:?}");
        let msg = err.to_string();
        assert!(
            msg.contains("--adapter software"),
            "message must name the escape hatch: {msg}"
        );
    }

    #[test]
    fn finding_no_vulkan_at_all_says_how_to_install_lavapipe() {
        for choice in [AdapterChoice::Auto, AdapterChoice::Software] {
            let msg = no_adapter(choice).to_string();
            assert!(
                msg.contains("mesa-vulkan-drivers"),
                "message must say how to install a software Vulkan driver: {msg}"
            );
        }
    }
}
