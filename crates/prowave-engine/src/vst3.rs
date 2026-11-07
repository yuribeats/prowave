//! macOS: hosting VST 3 plug-ins (Logic itself hosts only Audio Units; Pro Wave hosts both).
//!
//! A `.vst3` bundle is loaded with Core Foundation (`bundleEntry`, `GetPluginFactory`,
//! `bundleExit`); its factory lists the classes in it. Scanning runs in a helper process (see
//! the app's `vst3_scan.rs`) so a plug-in that crashes while it loads cannot take Pro Wave
//! with it.
//!
//! An instance is made on the UI thread ([`Vst3Instance::new`]): the component and its audio
//! processor, the edit controller (the same object, or a second one connected through
//! `IConnectionPoint`), 32-bit float stereo buses (mono when the plug-in only takes mono),
//! active and processing. It then moves to the audio thread as a
//! [`crate::BlockProcessor`]: each block gets the transport ([`crate::BlockTransport`] as a
//! `ProcessContext`), the track's notes as an `IEventList` and parameter changes as
//! `IParameterChanges`, all in memory made up front. Its window and settings stay reachable
//! from the UI thread through a [`Vst3Handle`], like an Audio Unit's `AuHandle`.
//!
//! Bindings: the `vst3` crate (coupler-rs/vst3-rs, MIT OR Apache-2.0), generated from the
//! VST 3 SDK, which Steinberg relicensed under MIT in 2025 (see `vst3_host.rs`).
use crate::plugin_track::BlockTransport;
use crate::vst3_host::{
    ComponentHandler, EditQueue, EventList, HostApp, MemoryStream, ParamChanges,
};
use objc2_core_foundation::{CFBundle, CFRetained, CFString, CFURL};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use vst3::Steinberg::Vst::{
    AudioBusBuffers, BusDirections_::kInput, BusDirections_::kOutput, Event, IAudioProcessor,
    IAudioProcessorTrait, IComponent, IComponentHandler, IComponentTrait, IConnectionPoint,
    IConnectionPointTrait, IEditController, IEditControllerTrait, IEventList, IMidiMapping,
    IMidiMappingTrait, IParameterChanges, MediaTypes_::kAudio, MediaTypes_::kEvent, ParamID,
    ParameterInfo, ProcessContext, ProcessData, ProcessModes_::kRealtime, ProcessSetup,
    SpeakerArrangement, SymbolicSampleSizes_::kSample32,
};
use vst3::Steinberg::{
    kResultOk, FUnknown, IBStream, IPluginBaseTrait, IPluginFactory, IPluginFactory2,
    IPluginFactory2Trait, IPluginFactoryTrait, PClassInfo, PClassInfo2, PFactoryInfo, TUID,
};
use vst3::{ComPtr, ComWrapper, Interface};

/// The category of the classes that process audio (the VST 3 SDK's `kVstAudioEffectClass`).
pub const AUDIO_MODULE_CLASS: &str = "Audio Module Class";

/// What a project saves for a VST 3: `vst3:` and the class id in hex.
pub const ID_PREFIX: &str = "vst3:";

/// One plug-in class in a `.vst3` bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vst3Class {
    pub cid: [u8; 16],
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub sdk_version: String,
    /// `|`-separated, e.g. `Instrument|Synth` or `Fx|Delay`.
    pub sub_categories: String,
}

impl Vst3Class {
    /// Plays notes (its sub-categories say `Instrument`).
    pub fn instrument(&self) -> bool {
        self.sub_categories
            .split('|')
            .any(|c| c.trim().eq_ignore_ascii_case("Instrument"))
    }
    /// The id a project saves: `vst3:` and the class id.
    pub fn id(&self) -> String {
        format!("{ID_PREFIX}{}", cid_hex(&self.cid))
    }
}

/// A class id as 32 hex digits, in the byte order the plug-in gives it.
pub fn cid_hex(cid: &[u8; 16]) -> String {
    cid.iter().map(|b| format!("{b:02X}")).collect()
}

/// The class id in a `vst3:` id (or in bare hex).
pub fn parse_id(id: &str) -> Option<[u8; 16]> {
    let hex = id.strip_prefix(ID_PREFIX).unwrap_or(id);
    if hex.len() != 32 || !hex.is_ascii() {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Whether `id` names a VST 3 class.
pub fn is_vst3_id(id: &str) -> bool {
    id.starts_with(ID_PREFIX)
}

fn text_of(c: &[std::ffi::c_char]) -> String {
    let bytes: Vec<u8> = c
        .iter()
        .take_while(|b| **b != 0)
        .map(|b| *b as u8)
        .collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

fn tuid(cid: &[u8; 16]) -> TUID {
    cid.map(|b| b as std::ffi::c_char)
}

// ---------------------------------------------------------------- the module

type EntryFn = unsafe extern "C" fn(*mut c_void) -> bool;
type FactoryFn = unsafe extern "system" fn() -> *mut IPluginFactory;

/// A loaded `.vst3` bundle and its factory. Its code stays loaded (see [`any_loaded`]).
pub struct Module {
    path: PathBuf,
    _bundle: CFRetained<CFBundle>,
    factory: Option<ComPtr<IPluginFactory>>,
}

// SAFETY: the factory and bundle are used from the UI thread only (instances are made there);
// the module sits in a shared cache and is never unloaded while the program runs.
unsafe impl Send for Module {}
// SAFETY: as above.
unsafe impl Sync for Module {}

impl Module {
    /// Loads the bundle at `path` and calls its `bundleEntry`. Can take seconds.
    pub fn load(path: &Path) -> Result<Self, String> {
        let shown = path.display();
        let url = CFURL::from_directory_path(path).ok_or_else(|| format!("{shown}: bad path"))?;
        let bundle =
            CFBundle::new(None, Some(&url)).ok_or_else(|| format!("{shown}: not a bundle"))?;
        LOADED.store(true, std::sync::atomic::Ordering::Relaxed);
        // SAFETY: loading runs the bundle's initializers; that is what hosting it means.
        if !unsafe { bundle.load_executable() } {
            return Err(format!("{shown}: its code did not load"));
        }
        let symbol = |name: &str| {
            let p = bundle.function_pointer_for_name(Some(&CFString::from_str(name)));
            (!p.is_null()).then_some(p)
        };
        let entry = symbol("bundleEntry").or_else(|| symbol("BundleEntry"));
        let factory = symbol("GetPluginFactory")
            .ok_or_else(|| format!("{shown}: no GetPluginFactory (not a VST 3)"))?;
        // SAFETY: the symbols have the signatures the VST 3 SDK gives them.
        unsafe {
            if let Some(entry) = entry {
                let entry: EntryFn = std::mem::transmute::<*mut c_void, EntryFn>(entry);
                let raw: *const CFBundle = &*bundle;
                if !entry(raw.cast_mut().cast()) {
                    return Err(format!("{shown}: bundleEntry failed"));
                }
            }
            let get: FactoryFn = std::mem::transmute::<*mut c_void, FactoryFn>(factory);
            let factory = ComPtr::from_raw(get());
            let module = Self {
                path: path.to_path_buf(),
                _bundle: bundle,
                factory,
            };
            if module.factory.is_none() {
                return Err(format!("{shown}: its factory is missing"));
            }
            Ok(module)
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn factory(&self) -> Result<&ComPtr<IPluginFactory>, String> {
        self.factory
            .as_ref()
            .ok_or_else(|| "the factory is gone".to_string())
    }

    /// The factory's vendor.
    pub fn vendor(&self) -> String {
        let Ok(f) = self.factory() else {
            return String::new();
        };
        // SAFETY: an all-zero PFactoryInfo is valid; the plug-in fills it.
        unsafe {
            let mut info: PFactoryInfo = std::mem::zeroed();
            if f.getFactoryInfo(&mut info) == kResultOk {
                text_of(&info.vendor)
            } else {
                String::new()
            }
        }
    }

    /// The audio classes in the bundle (instruments and effects).
    pub fn classes(&self) -> Vec<Vst3Class> {
        let Ok(f) = self.factory() else {
            return Vec::new();
        };
        let vendor = self.vendor();
        let f2 = f.cast::<IPluginFactory2>();
        let mut out = Vec::new();
        // SAFETY: plain factory calls with zeroed out-structs the plug-in fills.
        unsafe {
            for i in 0..f.countClasses().max(0) {
                let class = if let Some(f2) = &f2 {
                    let mut c: PClassInfo2 = std::mem::zeroed();
                    if f2.getClassInfo2(i, &mut c) != kResultOk {
                        continue;
                    }
                    Vst3Class {
                        cid: c.cid.map(|b| b as u8),
                        name: text_of(&c.name),
                        vendor: text_of(&c.vendor),
                        version: text_of(&c.version),
                        sdk_version: text_of(&c.sdkVersion),
                        sub_categories: format!(
                            "{}\u{0}{}",
                            text_of(&c.category),
                            text_of(&c.subCategories)
                        ),
                    }
                } else {
                    let mut c: PClassInfo = std::mem::zeroed();
                    if f.getClassInfo(i, &mut c) != kResultOk {
                        continue;
                    }
                    Vst3Class {
                        cid: c.cid.map(|b| b as u8),
                        name: text_of(&c.name),
                        vendor: String::new(),
                        version: String::new(),
                        sdk_version: String::new(),
                        sub_categories: format!("{}\u{0}", text_of(&c.category)),
                    }
                };
                // The category rode along in front of the sub-categories.
                let (category, subs) = class
                    .sub_categories
                    .split_once('\u{0}')
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .unwrap_or_default();
                if category != AUDIO_MODULE_CLASS {
                    continue;
                }
                out.push(Vst3Class {
                    vendor: if class.vendor.is_empty() {
                        vendor.clone()
                    } else {
                        class.vendor
                    },
                    sub_categories: subs,
                    ..class
                });
            }
        }
        out
    }

    fn create<I: Interface>(&self, cid: &[u8; 16]) -> Option<ComPtr<I>> {
        let f = self.factory().ok()?;
        let cid = tuid(cid);
        let iid = tuid(&I::IID);
        let mut obj: *mut c_void = std::ptr::null_mut();
        // SAFETY: the factory writes an owned reference of interface `I` to `obj`.
        unsafe {
            if f.createInstance(cid.as_ptr(), iid.as_ptr(), &mut obj) != kResultOk {
                return None;
            }
            ComPtr::from_raw(obj.cast::<I>())
        }
    }
}

// A module is never unloaded and never gets its `bundleExit`: Native Instruments' Qt-based
// plug-ins crash in their teardown (`QApplication::~QApplication` in a static destructor),
// whether or not `bundleExit` ran first. A process that loaded one ends with `_exit`, which
// skips static destructors (see [`any_loaded`] and the app's `vst3_scan::exit_now`).

/// The modules instances were made from; kept loaded while the program runs.
static MODULES: Mutex<Vec<Arc<Module>>> = Mutex::new(Vec::new());

/// Set once any VST 3 code has been loaded into this process.
static LOADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether this process has loaded VST 3 code; it should then end with `_exit` (skipping the
/// plug-ins' static destructors) rather than a normal exit.
pub fn any_loaded() -> bool {
    LOADED.load(std::sync::atomic::Ordering::Relaxed)
}

/// The module at `path`, loading it the first time.
pub fn module(path: &Path) -> Result<Arc<Module>, String> {
    if let Some(m) = MODULES
        .lock()
        .ok()
        .and_then(|m| m.iter().find(|m| m.path == path).cloned())
    {
        return Ok(m);
    }
    let m = Arc::new(Module::load(path)?);
    if let Ok(mut all) = MODULES.lock() {
        all.push(m.clone());
    }
    Ok(m)
}

/// Loads the bundle at `path` and lists its audio classes (for the scanner, which then ends
/// its process with `_exit`; the module stays loaded).
pub fn scan_bundle(path: &Path) -> Result<Vec<Vst3Class>, String> {
    Ok(module(path)?.classes())
}

/// The `.vst3` bundles in `dir` and its sub-folders (vendors put theirs in folders). A
/// folder that is itself a bundle's contents (a broken install that unpacked a bundle into
/// the VST3 folder) counts as a bundle too.
pub fn find_bundles(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if dir.join("Contents").join("MacOS").is_dir() && dir.extension().is_none() {
        out.push(dir.to_path_buf());
    }
    walk(dir, 0, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        if p.extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("vst3"))
        {
            out.push(p);
        } else if depth < 3 && p.file_name().is_some_and(|n| n != "Contents") {
            walk(&p, depth + 1, out);
        }
    }
}

/// The folders macOS keeps VST 3 plug-ins in: the user's, then the system's.
pub fn plugin_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        v.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/VST3"));
    }
    v.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
    v
}

// ---------------------------------------------------------------- the instance

/// The parts of a running plug-in both threads reach: the component, the controller and the
/// host objects they were given. Torn down when the last of [`Vst3Instance`] and its
/// [`Vst3Handle`]s goes.
struct Shared {
    name: String,
    component: ComPtr<IComponent>,
    controller: Option<ComPtr<IEditController>>,
    /// The controller is a second object (connected to the component).
    separate: bool,
    connection: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
    _host: ComWrapper<HostApp>,
    handler: ComWrapper<ComponentHandler>,
    param_ids: Vec<ParamID>,
    _module: Arc<Module>,
}

// SAFETY: VST 3 lets the host call a component and controller from the UI thread while the
// audio thread processes; the processor side only uses `IAudioProcessor`.
unsafe impl Send for Shared {}
// SAFETY: as above.
unsafe impl Sync for Shared {}

impl Drop for Shared {
    fn drop(&mut self) {
        // SAFETY: the SDK's teardown order: disconnect, then terminate controller, component.
        unsafe {
            if let Some((a, b)) = &self.connection {
                a.disconnect(b.as_ptr());
                b.disconnect(a.as_ptr());
            }
            if let Some(c) = &self.controller {
                c.setComponentHandler(std::ptr::null_mut());
                if self.separate {
                    c.terminate();
                }
            }
            self.component.terminate();
        }
    }
}

/// Events one block can hold; more are dropped.
pub const MAX_EVENTS: usize = 1024;
/// Parameters that can change in one block, and points each.
pub const MAX_PARAM_QUEUES: usize = 128;
pub const MAX_PARAM_POINTS: usize = 16;
/// MIDI controllers a plug-in can map (CC 0..127, aftertouch, pitch bend, program).
const CTRLS: usize = 131;

/// One audio bus's channels: the buffers and the pointers the plug-in reads.
struct Bus {
    bufs: Vec<Vec<f32>>,
    ptrs: Vec<*mut f32>,
}

impl Bus {
    fn new(channels: usize, frames: usize) -> Self {
        let mut bufs: Vec<Vec<f32>> = (0..channels).map(|_| vec![0f32; frames]).collect();
        let ptrs = bufs.iter_mut().map(|b| b.as_mut_ptr()).collect();
        Self { bufs, ptrs }
    }
}

/// One running VST 3 plug-in. Make it on the UI thread, process it on the audio thread, drop
/// it on the UI thread.
pub struct Vst3Instance {
    shared: Arc<Shared>,
    processor: ComPtr<IAudioProcessor>,
    instrument: bool,
    ins: Vec<Bus>,
    outs: Vec<Bus>,
    in_bufs: Vec<AudioBusBuffers>,
    out_bufs: Vec<AudioBusBuffers>,
    events: ComWrapper<EventList>,
    out_events: ComWrapper<EventList>,
    params: ComWrapper<ParamChanges>,
    out_params: ComWrapper<ParamChanges>,
    edits: EditQueue,
    has_event_bus: bool,
    /// MIDI controller `n` on channel `c` (index `c * CTRLS + n`) sets this parameter.
    midi_map: Vec<Option<ParamID>>,
    context: ProcessContext,
    transport: BlockTransport,
    continuous: i64,
    sample_rate: f64,
    max_frames: usize,
    latency_s: f64,
    errors: u64,
}

// SAFETY: a VST 3 processor may be used from another thread than the one that made it, one
// thread at a time; the engine hands it over through a queue (as with Audio Units).
unsafe impl Send for Vst3Instance {}

fn channels_of(arr: SpeakerArrangement) -> usize {
    arr.count_ones() as usize
}

const STEREO: SpeakerArrangement = vst3::Steinberg::Vst::SpeakerArr::kStereo;
const MONO: SpeakerArrangement = vst3::Steinberg::Vst::SpeakerArr::kMono;

impl Vst3Instance {
    /// Opens class `cid` of the bundle at `path` at `sample_rate` for blocks of up to
    /// `max_frames`. Can take seconds (sample libraries, license checks).
    pub fn new(
        path: &Path,
        cid: &[u8; 16],
        sample_rate: u32,
        max_frames: usize,
    ) -> Result<Self, String> {
        let module = module(path)?;
        let class = module.classes().into_iter().find(|c| &c.cid == cid);
        let name = class
            .as_ref()
            .map_or_else(|| cid_hex(cid), |c| format!("{}: {}", c.vendor, c.name));
        let component = module
            .create::<IComponent>(cid)
            .ok_or_else(|| format!("{name} did not open"))?;
        let host = ComWrapper::new(HostApp);
        let host_ptr = host
            .as_com_ref::<vst3::Steinberg::Vst::IHostApplication>()
            .map_or(std::ptr::null_mut(), |r| r.as_ptr())
            .cast::<FUnknown>();
        // SAFETY: the VST 3 set-up sequence on objects made here, on the UI thread.
        unsafe {
            if component.initialize(host_ptr) != kResultOk {
                return Err(format!("{name} did not start"));
            }
            let processor = component.cast::<IAudioProcessor>();
            let Some(processor) = processor else {
                component.terminate();
                return Err(format!("{name} does not process audio"));
            };
            // The controller: the component itself, or a class of its own.
            let mut separate = false;
            let mut controller = component.cast::<IEditController>();
            if controller.is_none() {
                let mut ccid: TUID = [0; 16];
                if component.getControllerClassId(&mut ccid) == kResultOk {
                    let ccid = ccid.map(|b| b as u8);
                    if let Some(c) = module.create::<IEditController>(&ccid) {
                        if c.initialize(host_ptr) == kResultOk {
                            separate = true;
                            controller = Some(c);
                        }
                    }
                }
            }
            let (handler, edits) = ComponentHandler::new(1024);
            let handler = ComWrapper::new(handler);
            let mut connection = None;
            if let Some(c) = &controller {
                if let Some(h) = handler.as_com_ref::<IComponentHandler>() {
                    c.setComponentHandler(h.as_ptr());
                }
                if separate {
                    if let (Some(a), Some(b)) = (
                        component.cast::<IConnectionPoint>(),
                        c.cast::<IConnectionPoint>(),
                    ) {
                        a.connect(b.as_ptr());
                        b.connect(a.as_ptr());
                        connection = Some((a, b));
                    }
                }
            }
            let mut param_ids = Vec::new();
            if let Some(c) = &controller {
                for i in 0..c.getParameterCount().max(0) {
                    let mut info: ParameterInfo = std::mem::zeroed();
                    if c.getParameterInfo(i, &mut info) == kResultOk {
                        param_ids.push(info.id);
                    }
                }
            }
            let shared = Arc::new(Shared {
                name: name.clone(),
                component,
                controller,
                separate,
                connection,
                _host: host,
                handler,
                param_ids,
                _module: module,
            });
            // The controller learns the component's settings.
            if let Some(c) = &shared.controller {
                let s = ComWrapper::new(MemoryStream::default());
                if let Some(sp) = s.as_com_ref::<IBStream>() {
                    if shared.component.getState(sp.as_ptr()) == kResultOk {
                        s.rewind();
                        c.setComponentState(sp.as_ptr());
                    }
                }
            }
            let instrument = class.is_some_and(|c| c.instrument());
            Self::set_up(
                shared,
                processor,
                edits,
                instrument,
                sample_rate,
                max_frames,
            )
        }
    }

    unsafe fn set_up(
        shared: Arc<Shared>,
        processor: ComPtr<IAudioProcessor>,
        edits: EditQueue,
        instrument_class: bool,
        sample_rate: u32,
        max_frames: usize,
    ) -> Result<Self, String> {
        let name = shared.name.clone();
        let comp = &shared.component;
        let n_in = comp.getBusCount(kAudio as i32, kInput as i32).max(0) as usize;
        let n_out = comp.getBusCount(kAudio as i32, kOutput as i32).max(0) as usize;
        let n_ev = comp.getBusCount(kEvent as i32, kInput as i32).max(0) as usize;
        if n_out == 0 {
            return Err(format!("{name} has no audio output"));
        }
        let current = |dir: u32, n: usize| -> Vec<SpeakerArrangement> {
            (0..n)
                .map(|i| {
                    let mut a: SpeakerArrangement = 0;
                    if processor.getBusArrangement(dir as i32, i as i32, &mut a) == kResultOk {
                        a
                    } else {
                        STEREO
                    }
                })
                .collect()
        };
        let mut ins = current(kInput, n_in);
        let mut outs = current(kOutput, n_out);
        // Main buses stereo; a plug-in that only takes mono gets mono.
        for (i_arr, o_arr) in [
            (STEREO, STEREO),
            (MONO, MONO),
            (MONO, STEREO),
            (STEREO, MONO),
        ] {
            if let Some(a) = ins.first_mut() {
                *a = i_arr;
            }
            outs[0] = o_arr;
            let r = processor.setBusArrangements(
                ins.as_mut_ptr(),
                n_in as i32,
                outs.as_mut_ptr(),
                n_out as i32,
            );
            if r == kResultOk {
                break;
            }
        }
        // What the plug-in settled on is what it gets.
        let ins_now = current(kInput, n_in);
        let outs_now = current(kOutput, n_out);
        let main_out = channels_of(outs_now[0]);
        if main_out == 0 {
            return Err(format!("{name} has an empty output bus"));
        }
        for i in 0..n_in {
            comp.activateBus(kAudio as i32, kInput as i32, i as i32, u8::from(i == 0));
        }
        for i in 0..n_out {
            comp.activateBus(kAudio as i32, kOutput as i32, i as i32, u8::from(i == 0));
        }
        for i in 0..n_ev {
            comp.activateBus(kEvent as i32, kInput as i32, i as i32, u8::from(i == 0));
        }
        let max_frames = max_frames.max(64);
        let mut setup = ProcessSetup {
            processMode: kRealtime as i32,
            symbolicSampleSize: kSample32 as i32,
            maxSamplesPerBlock: max_frames as i32,
            sampleRate: f64::from(sample_rate),
        };
        if processor.canProcessSampleSize(kSample32 as i32) != kResultOk {
            return Err(format!("{name} cannot process 32-bit float"));
        }
        if processor.setupProcessing(&mut setup) != kResultOk {
            return Err(format!("{name} refused {sample_rate} Hz"));
        }
        if comp.setActive(1) != kResultOk {
            return Err(format!("{name} did not activate"));
        }
        let latency = processor.getLatencySamples();
        // Many plug-ins answer "not implemented" here; that is fine.
        processor.setProcessing(1);
        let mut midi_map = vec![None; 16 * CTRLS];
        if let Some(mm) = shared
            .controller
            .as_ref()
            .and_then(|c| c.cast::<IMidiMapping>())
        {
            for ch in 0..16i16 {
                for n in 0..CTRLS as i16 {
                    let mut id: ParamID = 0;
                    if mm.getMidiControllerAssignment(0, ch, n, &mut id) == kResultOk {
                        midi_map[ch as usize * CTRLS + n as usize] = Some(id);
                    }
                }
            }
        }
        let bus_of = |arr: &SpeakerArrangement| Bus::new(channels_of(*arr).min(64), max_frames);
        let mut inst = Self {
            ins: ins_now.iter().map(bus_of).collect(),
            outs: outs_now.iter().map(bus_of).collect(),
            in_bufs: Vec::new(),
            out_bufs: Vec::new(),
            events: ComWrapper::new(EventList::with_capacity(MAX_EVENTS)),
            out_events: ComWrapper::new(EventList::with_capacity(MAX_EVENTS)),
            params: ComWrapper::new(ParamChanges::new(MAX_PARAM_QUEUES, MAX_PARAM_POINTS)),
            out_params: ComWrapper::new(ParamChanges::new(MAX_PARAM_QUEUES, MAX_PARAM_POINTS)),
            edits,
            has_event_bus: n_ev > 0,
            instrument: instrument_class || n_in == 0,
            midi_map,
            context: std::mem::zeroed(),
            transport: BlockTransport::default(),
            continuous: 0,
            sample_rate: f64::from(sample_rate),
            max_frames,
            latency_s: f64::from(latency) / f64::from(sample_rate.max(1)),
            errors: 0,
            processor,
            shared,
        };
        inst.in_bufs = inst.ins.iter_mut().map(bus_buffers).collect();
        inst.out_bufs = inst.outs.iter_mut().map(bus_buffers).collect();
        Ok(inst)
    }
}

fn bus_buffers(b: &mut Bus) -> AudioBusBuffers {
    // SAFETY: an all-zero AudioBusBuffers is valid; the pointer array lives in `b`.
    let mut ab: AudioBusBuffers = unsafe { std::mem::zeroed() };
    ab.numChannels = b.ptrs.len() as i32;
    ab.__field0.channelBuffers32 = b.ptrs.as_mut_ptr();
    ab
}

/// The VST 3 `ProcessContext` for a block that starts at `t`. `continuous` counts the frames
/// processed since the plug-in started. No allocation.
pub fn fill_context(
    t: &BlockTransport,
    sample_rate: f64,
    continuous: i64,
    out: &mut ProcessContext,
) {
    use vst3::Steinberg::Vst::ProcessContext_::StatesAndFlags_::*;
    let mut state =
        kTempoValid | kTimeSigValid | kProjectTimeMusicValid | kBarPositionValid | kContTimeValid;
    if t.playing {
        state |= kPlaying;
    }
    if t.recording {
        state |= kRecording;
    }
    if let Some((s, e)) = t.cycle {
        state |= kCycleValid | kCycleActive;
        out.cycleStartMusic = s;
        out.cycleEndMusic = e;
    } else {
        out.cycleStartMusic = 0.0;
        out.cycleEndMusic = 0.0;
    }
    out.state = state;
    out.sampleRate = sample_rate;
    out.projectTimeSamples = t.sample_pos;
    out.continousTimeSamples = continuous;
    out.projectTimeMusic = t.beat;
    out.barPositionMusic = t.bar_start_beat;
    out.tempo = t.tempo;
    out.timeSigNumerator = i32::from(t.sig.0.max(1));
    out.timeSigDenominator = i32::from(t.sig.1.max(1));
}

/// A note-on or note-off as a VST 3 event, or `None` for other messages.
pub fn note_event(status: u8, data1: u8, data2: u8, offset: u32) -> Option<Event> {
    use vst3::Steinberg::Vst::Event_::EventTypes_::{kNoteOffEvent, kNoteOnEvent};
    let kind = status & 0xf0;
    let channel = i16::from(status & 0x0f);
    // SAFETY: an all-zero Event is valid plain data.
    let mut e: Event = unsafe { std::mem::zeroed() };
    e.busIndex = 0;
    e.sampleOffset = offset.min(i32::MAX as u32) as i32;
    match kind {
        0x90 if data2 > 0 => {
            e.r#type = kNoteOnEvent as u16;
            e.__field0.noteOn = vst3::Steinberg::Vst::NoteOnEvent {
                channel,
                pitch: i16::from(data1 & 0x7f),
                tuning: 0.0,
                velocity: f32::from(data2) / 127.0,
                length: 0,
                noteId: -1,
            };
        }
        0x80 | 0x90 => {
            e.r#type = kNoteOffEvent as u16;
            e.__field0.noteOff = vst3::Steinberg::Vst::NoteOffEvent {
                channel,
                pitch: i16::from(data1 & 0x7f),
                velocity: if kind == 0x80 {
                    f32::from(data2) / 127.0
                } else {
                    0.0
                },
                noteId: -1,
                tuning: 0.0,
            };
        }
        _ => return None,
    }
    Some(e)
}

impl Vst3Instance {
    /// Plays notes (an instrument, or an effect that takes MIDI).
    pub fn is_instrument(&self) -> bool {
        self.instrument
    }

    /// A handle for the plug-in's window and settings, used from the UI thread while the
    /// instance renders on the audio thread.
    pub fn handle(&self) -> Vst3Handle {
        Vst3Handle(self.shared.clone())
    }

    /// The delay the plug-in puts on the sound, in seconds (reported when it was activated).
    pub fn latency_seconds(&self) -> f64 {
        self.latency_s
    }

    /// Blocks the plug-in failed to process (they were silent).
    pub fn errors(&self) -> u64 {
        self.errors
    }

    /// How many parameters the plug-in has (what [`Vst3Instance::set_parameter`] indexes).
    pub fn parameter_count(&self) -> usize {
        self.shared.param_ids.len()
    }

    /// A MIDI message `offset` frames into the next block: notes become events, controllers
    /// the parameters the plug-in maps them to. Audio thread, no allocation.
    pub fn midi(&mut self, status: u8, data1: u8, data2: u8, offset: u32) {
        if let Some(e) = note_event(status, data1, data2, offset) {
            if self.has_event_bus {
                self.events.push(e);
            }
            return;
        }
        let ch = usize::from(status & 0x0f);
        let (ctrl, value) = match status & 0xf0 {
            0xb0 => (usize::from(data1 & 0x7f), f64::from(data2) / 127.0),
            0xd0 => (128, f64::from(data1) / 127.0),
            0xe0 => (
                129,
                f64::from((u16::from(data2) << 7) | u16::from(data1)) / 16383.0,
            ),
            _ => return,
        };
        if let Some(Some(id)) = self.midi_map.get(ch * CTRLS + ctrl) {
            self.params.push(*id, offset as i32, value);
        }
    }

    /// Parameter `index` (in the controller's order) to `value`, normalized 0..1, at the
    /// start of the next block. Audio thread, no allocation.
    pub fn set_parameter(&mut self, index: usize, value: f32) {
        self.set_parameter_at(index, value, 0);
    }

    /// [`Vst3Instance::set_parameter`] from frame `offset` of the next block (VST 3 takes
    /// sample-accurate parameter changes). Audio thread, no allocation.
    pub fn set_parameter_at(&mut self, index: usize, value: f32, offset: u32) {
        if let Some(id) = self.shared.param_ids.get(index) {
            if value.is_finite() {
                self.params
                    .push(*id, offset as i32, f64::from(value.clamp(0.0, 1.0)));
            }
        }
    }

    /// Renders one block: `input` (an effect's; silence for instruments) into `out`. Audio
    /// thread, no allocation.
    pub fn process(&mut self, input: (&[f32], &[f32]), out: (&mut [f32], &mut [f32])) {
        let frames = out.0.len().min(out.1.len());
        // What the plug-in's window changed since the last block.
        while let Ok((id, v)) = self.edits.pop() {
            self.params.push(id, 0, v);
        }
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(self.max_frames);
            if let Some(bus) = self.ins.first_mut() {
                let (il, ir) = input;
                let get = |s: &[f32], i: usize| s.get(i).copied().unwrap_or(0.0);
                match bus.bufs.len() {
                    0 => {}
                    1 => {
                        for i in 0..n {
                            bus.bufs[0][i] = 0.5 * (get(il, done + i) + get(ir, done + i));
                        }
                    }
                    _ => {
                        for i in 0..n {
                            bus.bufs[0][i] = get(il, done + i);
                            bus.bufs[1][i] = get(ir, done + i);
                        }
                    }
                }
            }
            let ok = self.run(n, done);
            let main = &self.outs[0].bufs;
            for i in 0..n {
                let l = if ok { main[0][i] } else { 0.0 };
                let r = if ok {
                    main.get(1).map_or(l, |b| b[i])
                } else {
                    0.0
                };
                out.0[done + i] = l;
                out.1[done + i] = r;
            }
            done += n;
        }
    }

    fn run(&mut self, n: usize, done: usize) -> bool {
        let mut t = self.transport;
        t.sample_pos += done as i64;
        fill_context(&t, self.sample_rate, self.continuous, &mut self.context);
        let params = self
            .params
            .as_com_ref::<IParameterChanges>()
            .map_or(std::ptr::null_mut(), |r| r.as_ptr());
        let out_params = self
            .out_params
            .as_com_ref::<IParameterChanges>()
            .map_or(std::ptr::null_mut(), |r| r.as_ptr());
        let events = if self.has_event_bus {
            self.events
                .as_com_ref::<IEventList>()
                .map_or(std::ptr::null_mut(), |r| r.as_ptr())
        } else {
            std::ptr::null_mut()
        };
        let out_events = self
            .out_events
            .as_com_ref::<IEventList>()
            .map_or(std::ptr::null_mut(), |r| r.as_ptr());
        for b in self.in_bufs.iter_mut().chain(self.out_bufs.iter_mut()) {
            b.silenceFlags = 0;
        }
        let mut data = ProcessData {
            processMode: kRealtime as i32,
            symbolicSampleSize: kSample32 as i32,
            numSamples: n as i32,
            numInputs: self.in_bufs.len() as i32,
            numOutputs: self.out_bufs.len() as i32,
            inputs: if self.in_bufs.is_empty() {
                std::ptr::null_mut()
            } else {
                self.in_bufs.as_mut_ptr()
            },
            outputs: self.out_bufs.as_mut_ptr(),
            inputParameterChanges: params,
            outputParameterChanges: out_params,
            inputEvents: events,
            outputEvents: out_events,
            processContext: &mut self.context,
        };
        // SAFETY: every pointer in `data` points at memory this instance owns for the call;
        // the buffers hold at least `n` frames (n <= max_frames).
        let status = unsafe { self.processor.process(&mut data) };
        self.continuous += n as i64;
        self.events.clear();
        self.params.clear();
        self.out_events.clear();
        self.out_params.clear();
        if status != kResultOk {
            self.errors += 1;
            return false;
        }
        true
    }
}

impl Drop for Vst3Instance {
    fn drop(&mut self) {
        // SAFETY: the SDK's order: stop processing, deactivate; `Shared` terminates.
        unsafe {
            self.processor.setProcessing(0);
            self.shared.component.setActive(0);
        }
    }
}

impl crate::plugin_track::BlockProcessor for Vst3Instance {
    fn midi(&mut self, status: u8, data1: u8, data2: u8, offset: u32) {
        Vst3Instance::midi(self, status, data1, data2, offset);
    }
    fn process(&mut self, input: (&[f32], &[f32]), out: (&mut [f32], &mut [f32])) {
        Vst3Instance::process(self, input, out);
    }
    fn latency_seconds(&self) -> f64 {
        self.latency_s
    }
    fn set_parameter(&mut self, index: usize, value: f32) {
        Vst3Instance::set_parameter(self, index, value);
    }
    fn takes_param_offsets(&self) -> bool {
        true
    }
    fn set_parameter_at(&mut self, index: u32, value: f32, offset: u32) {
        Vst3Instance::set_parameter_at(self, index as usize, value, offset);
    }
    fn block_transport(&mut self, t: &BlockTransport) {
        self.transport = *t;
    }
}

// ---------------------------------------------------------------- the handle

/// The UI thread's way to a [`Vst3Instance`] the audio thread owns: its window and its
/// settings. It keeps the plug-in's component and controller alive.
#[derive(Clone)]
pub struct Vst3Handle(Arc<Shared>);

/// The start of Pro Wave's VST 3 settings: the component's and then the controller's bytes,
/// each after its length (u32, little-endian).
const STATE_MAGIC: &[u8; 4] = b"PWV3";

/// Packs the component's and the controller's settings into one blob.
pub fn pack_state(component: &[u8], controller: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(12 + component.len() + controller.len());
    v.extend_from_slice(STATE_MAGIC);
    v.extend_from_slice(&(component.len() as u32).to_le_bytes());
    v.extend_from_slice(component);
    v.extend_from_slice(&(controller.len() as u32).to_le_bytes());
    v.extend_from_slice(controller);
    v
}

/// The component's and the controller's settings from [`pack_state`].
pub fn unpack_state(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let rest = bytes.strip_prefix(STATE_MAGIC)?;
    let take = |b: &[u8]| -> Option<usize> {
        Some(u32::from_le_bytes(b.get(..4)?.try_into().ok()?) as usize)
    };
    let n = take(rest)?;
    let comp = rest.get(4..4 + n)?;
    let rest = &rest[4 + n..];
    let m = take(rest)?;
    let ctrl = rest.get(4..4 + m)?;
    Some((comp, ctrl))
}

impl Vst3Handle {
    /// "Vendor: Name".
    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// The plug-in's settings (component and controller), for saving in a project.
    pub fn state(&self) -> Option<Vec<u8>> {
        let s = &self.0;
        let comp = read_stream(|p| unsafe { s.component.getState(p) })?;
        let ctrl = s
            .controller
            .as_ref()
            .and_then(|c| read_stream(|p| unsafe { c.getState(p) }))
            .unwrap_or_default();
        Some(pack_state(&comp, &ctrl))
    }

    /// Restores settings from [`Vst3Handle::state`]: the component's, which the controller
    /// also learns, then the controller's own.
    pub fn set_state(&self, bytes: &[u8]) -> bool {
        let Some((comp, ctrl)) = unpack_state(bytes) else {
            return false;
        };
        let s = &self.0;
        let stream = ComWrapper::new(MemoryStream::from_bytes(comp));
        let Some(p) = stream.as_com_ref::<IBStream>().map(|r| r.as_ptr()) else {
            return false;
        };
        // SAFETY: setState reads our stream, which lives across the calls; UI thread.
        unsafe {
            if s.component.setState(p) != kResultOk {
                return false;
            }
            if let Some(c) = &s.controller {
                stream.rewind();
                c.setComponentState(p);
                if !ctrl.is_empty() {
                    let cs = ComWrapper::new(MemoryStream::from_bytes(ctrl));
                    if let Some(cp) = cs.as_com_ref::<IBStream>() {
                        c.setState(cp.as_ptr());
                    }
                }
            }
        }
        true
    }

    /// How many parameters the controller has.
    pub fn parameter_count(&self) -> usize {
        self.0.param_ids.len()
    }

    /// Parameter `index`'s title, whether the host may change it (automatable, not read
    /// only, not hidden, not the bypass or program switch) and its steps (0: continuous).
    pub fn parameter_info(&self, index: usize) -> Option<(String, bool, i32)> {
        use vst3::Steinberg::Vst::ParameterInfo_::ParameterFlags_::{
            kCanAutomate, kIsBypass, kIsHidden, kIsProgramChange, kIsReadOnly,
        };
        let c = self.0.controller.as_ref()?;
        // SAFETY: a UI-thread controller call into a zeroed struct it fills.
        unsafe {
            let mut info: ParameterInfo = std::mem::zeroed();
            if c.getParameterInfo(index as i32, &mut info) != kResultOk {
                return None;
            }
            let f = info.flags;
            let free = f & kCanAutomate != 0
                && f & (kIsReadOnly | kIsHidden | kIsBypass | kIsProgramChange) == 0;
            Some((
                crate::vst3_host::read_string128(&info.title),
                free,
                info.stepCount.max(0),
            ))
        }
    }

    /// The parameters the user moved in the plug-in's window since the last call: (index in
    /// the controller's order, normalized value). UI thread.
    pub fn take_edits(&self) -> Vec<(usize, f64)> {
        let ids = &self.0.param_ids;
        self.0
            .handler
            .take_seen()
            .into_iter()
            .filter_map(|(id, v)| Some((ids.iter().position(|x| *x == id)?, v)))
            .collect()
    }

    /// Parameter `index`'s normalized value as the controller has it.
    pub fn parameter(&self, index: usize) -> Option<f64> {
        let id = *self.0.param_ids.get(index)?;
        let c = self.0.controller.as_ref()?;
        // SAFETY: a UI-thread controller call.
        Some(unsafe { c.getParamNormalized(id) })
    }

    /// Shows parameter `index` at `value` in the controller (its window). The processor
    /// gets the change through [`Vst3Instance::set_parameter`].
    pub fn set_parameter_shown(&self, index: usize, value: f64) {
        if let (Some(id), Some(c)) = (self.0.param_ids.get(index), &self.0.controller) {
            // SAFETY: a UI-thread controller call.
            unsafe {
                c.setParamNormalized(*id, value.clamp(0.0, 1.0));
            }
        }
    }

    /// The `restartComponent` flags the plug-in raised since the last call (for example
    /// `kLatencyChanged`), and clears them.
    pub fn take_restart_flags(&self) -> i32 {
        self.0
            .handler
            .restart
            .swap(0, std::sync::atomic::Ordering::Relaxed)
    }

    /// Opens the plug-in's own window (its `IPlugView` in an `NSView`). Main thread only.
    pub fn open_window(&self, title: &str) -> Result<Vst3Window, String> {
        crate::vst3_view::open(self.clone(), title)
    }

    pub(crate) fn controller(&self) -> Option<&ComPtr<IEditController>> {
        self.0.controller.as_ref()
    }
}

/// Bytes a plug-in writes into a fresh stream through `f`.
fn read_stream(f: impl FnOnce(*mut IBStream) -> i32) -> Option<Vec<u8>> {
    let stream = ComWrapper::new(MemoryStream::default());
    let p = stream.as_com_ref::<IBStream>()?.as_ptr();
    (f(p) == kResultOk).then(|| stream.bytes())
}

pub use crate::vst3_view::{pump_main_loop, Vst3Window};

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        let cid = [
            0xAB, 0xCD, 0xEF, 0x01, 0x91, 0x82, 0xFA, 0xEB, 0x56, 0x55, 0x42, 0x5A, 0x56, 0x32,
            0x30, 0x32,
        ];
        let c = Vst3Class {
            cid,
            name: "VU202".into(),
            vendor: "vubeatz".into(),
            version: String::new(),
            sdk_version: String::new(),
            sub_categories: "Instrument|Synth".into(),
        };
        assert_eq!(c.id(), "vst3:ABCDEF019182FAEB5655425A56323032");
        assert_eq!(parse_id(&c.id()), Some(cid));
        assert!(c.instrument());
        assert!(is_vst3_id(&c.id()));
        assert_eq!(parse_id("vst3:XYZ"), None);
        let fx = Vst3Class {
            sub_categories: "Fx|Delay".into(),
            ..c
        };
        assert!(!fx.instrument());
    }

    #[test]
    fn state_packs_both_halves() {
        let b = pack_state(b"comp", b"controller");
        assert_eq!(unpack_state(&b), Some((&b"comp"[..], &b"controller"[..])));
        assert_eq!(unpack_state(b"bplist00"), None);
        assert_eq!(unpack_state(&b[..10]), None);
        let empty = pack_state(b"", b"");
        assert_eq!(unpack_state(&empty), Some((&b""[..], &b""[..])));
    }

    #[test]
    fn context_follows_the_transport_without_allocating() {
        use vst3::Steinberg::Vst::ProcessContext_::StatesAndFlags_::*;
        let t = BlockTransport {
            playing: true,
            recording: false,
            sample_pos: 96_000,
            beat: 5.5,
            bar_start_beat: 4.0,
            tempo: 90.0,
            sig: (6, 8),
            cycle: Some((4.0, 8.0)),
        };
        // SAFETY: zeroed plain data.
        let mut c: ProcessContext = unsafe { std::mem::zeroed() };
        assert_no_alloc::assert_no_alloc(|| fill_context(&t, 48_000.0, 777, &mut c));
        assert_eq!(c.projectTimeSamples, 96_000);
        assert_eq!(c.continousTimeSamples, 777);
        assert_eq!(c.projectTimeMusic, 5.5);
        assert_eq!(c.barPositionMusic, 4.0);
        assert_eq!(c.tempo, 90.0);
        assert_eq!((c.timeSigNumerator, c.timeSigDenominator), (6, 8));
        assert_eq!((c.cycleStartMusic, c.cycleEndMusic), (4.0, 8.0));
        let s = c.state;
        for f in [
            kPlaying,
            kTempoValid,
            kTimeSigValid,
            kCycleActive,
            kProjectTimeMusicValid,
        ] {
            assert_ne!(s & f, 0, "flag {f}");
        }
        assert_eq!(s & kRecording, 0);
        let stopped = BlockTransport {
            playing: false,
            cycle: None,
            ..t
        };
        fill_context(&stopped, 48_000.0, 0, &mut c);
        assert_eq!(c.state & kPlaying, 0);
        assert_eq!(c.state & kCycleActive, 0);
    }

    #[test]
    fn notes_become_events() {
        let on = note_event(0x91, 60, 127, 33).expect("note on");
        assert_eq!(on.sampleOffset, 33);
        // SAFETY: the union field matches the type just set.
        unsafe {
            assert_eq!(on.__field0.noteOn.pitch, 60);
            assert_eq!(on.__field0.noteOn.channel, 1);
            assert_eq!(on.__field0.noteOn.velocity, 1.0);
        }
        let off = note_event(0x90, 60, 0, 5).expect("velocity 0 is an off");
        assert_eq!(
            off.r#type,
            vst3::Steinberg::Vst::Event_::EventTypes_::kNoteOffEvent as u16
        );
        assert!(note_event(0xb0, 1, 2, 0).is_none());
    }
}
