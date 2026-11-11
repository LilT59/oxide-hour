//! Zero Hour RTS-UI groundwork: `.wnd` window layouts, `MappedImage` INI
//! blocks, and image -> texture / text resolution. No rendering.
//!
//! Ported from `Core/GameEngine/Source/GameClient/GUI/GameWindowManagerScript.cpp`
//! (the `.wnd` parser), `Core/GameEngine/Source/GameClient/System/Image.cpp` and
//! `Core/GameEngine/Include/GameClient/Image.h` (mapped images).
//!
//! * [`wnd::parse`] reads a `Window\*.wnd` file into a [`wnd::WndFile`] tree
//!   (FILE_VERSION, LAYOUTBLOCK, per-window fields, typed gadget data, children).
//! * [`ImageDb::from_ini`] collects every `MappedImage` from the merged
//!   [`zr_ini::IniDb`] with the engine's UV / rotated-size maths.
//! * [`ImageDb::resolve`] walks image -> texture and checks the VFS, and
//!   [`report::make_report`] summarises the shipped UI.
//!
//! Parsers take untrusted bytes: reads are bounds-checked and malformed input is
//! an error with a line number, never a panic.

pub mod command;
pub mod gadget;
pub mod image;
pub mod report;
pub mod resolve;
pub mod scheme;
pub mod value;
pub mod wnd;

pub use command::{command_option, BorderType, CommandButton, CommandDb, CommandKind, CommandSet};
pub use image::{image_status, ImageDb, MappedImage};
pub use report::{make_report, ImageStats, TextStats, UiReport, WndStats};
pub use resolve::{texture_candidates, ImageResolution};
pub use scheme::{ControlBarScheme, SchemeDb, SchemeImage};
pub use wnd::{
    parse, ComboBoxData, DrawData, DrawDataSets, FontSpec, GadgetData, LayoutBlock, ListBoxData,
    ParseError, RadioButtonData, Rgba, ScreenRect, SliderData, StaticTextData, TabControlData,
    TextColors, TextEntryData, TextState, Window, WindowDefaults, WndFile,
};
