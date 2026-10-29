//! Embedded monochrome icon set (24px grid, 1.8px strokes).

use gpui::{px, rgb, svg, Styled, Svg};

pub const FOLDER: &str = "icons/folder.svg";
pub const SAVE: &str = "icons/save.svg";
pub const REFRESH: &str = "icons/refresh.svg";
pub const PLAY: &str = "icons/play.svg";
pub const PAUSE: &str = "icons/pause.svg";
pub const PREV: &str = "icons/prev.svg";
pub const NEXT: &str = "icons/next.svg";
pub const KEYFRAME: &str = "icons/keyframe.svg";
pub const FILM: &str = "icons/film.svg";
pub const WARNING: &str = "icons/warning.svg";
pub const CHECK: &str = "icons/check.svg";
pub const CHEVRON_DOWN: &str = "icons/chevron-down.svg";
pub const SETTINGS: &str = "icons/settings.svg";
pub const CLOSE: &str = "icons/close.svg";

const FOLDER_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7.5A2.5 2.5 0 0 1 5.5 5h3.2l2 2.2h7.8A2.5 2.5 0 0 1 21 9.7v7.8a2.5 2.5 0 0 1-2.5 2.5h-13A2.5 2.5 0 0 1 3 17.5z"/></svg>"##;
const SAVE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M5 4h11l3 3v13H5z"/><path d="M8 4v6h7V4"/><path d="M8 20v-6h8v6"/></svg>"##;
const REFRESH_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M20 11a8 8 0 0 0-13.7-5.6L4 7.6"/><path d="M4 4v3.6h3.6"/><path d="M4 13a8 8 0 0 0 13.7 5.6L20 16.4"/><path d="M20 20v-3.6h-3.6"/></svg>"##;
const PLAY_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="#fff" d="M8.2 5.4c0-.8.9-1.3 1.5-.9l9 6.2c.6.4.6 1.3 0 1.7l-9 6.2c-.7.4-1.5-.1-1.5-.9z"/></svg>"##;
const PAUSE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect x="6.5" y="5" width="3.6" height="14" rx="1.2" fill="#fff"/><rect x="13.9" y="5" width="3.6" height="14" rx="1.2" fill="#fff"/></svg>"##;
const PREV_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M14.5 6.5 9 12l5.5 5.5"/></svg>"##;
const NEXT_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M9.5 6.5 15 12l-5.5 5.5"/></svg>"##;
const KEYFRAME_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="none" stroke="#fff" stroke-width="2" stroke-linejoin="round" d="M12 4.5 19.5 12 12 19.5 4.5 12z"/></svg>"##;
const FILM_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="3.5" y="5" width="17" height="14" rx="2"/><path d="M7.5 5v14M16.5 5v14M3.5 9.5h4M3.5 14.5h4M16.5 9.5h4M16.5 14.5h4"/></svg>"##;
const WARNING_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 4 21 20H3z"/><path d="M12 10v4.5"/><path d="M12 17.3v.2"/></svg>"##;
const CHECK_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m5 12.5 4.5 4.5L19 6.5"/></svg>"##;
const CHEVRON_DOWN_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="m6.5 9.5 5.5 5.5 5.5-5.5"/></svg>"##;
const SETTINGS_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M10.65 2.39L13.35 2.39L14.87 5.07L17.84 4.25L19.75 6.16L18.93 9.13L21.61 10.65L21.61 13.35L18.93 14.87L19.75 17.84L17.84 19.75L14.87 18.93L13.35 21.61L10.65 21.61L9.13 18.93L6.16 19.75L4.25 17.84L5.07 14.87L2.39 13.35L2.39 10.65L5.07 9.13L4.25 6.16L6.16 4.25L9.13 5.07Z"/><circle cx="12" cy="12" r="3.4"/></svg>"##;
const CLOSE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M6.5 6.5l11 11M17.5 6.5l-11 11"/></svg>"##;

pub fn asset(path: &str) -> Option<&'static [u8]> {
    Some(match path {
        FOLDER => FOLDER_SVG,
        SAVE => SAVE_SVG,
        REFRESH => REFRESH_SVG,
        PLAY => PLAY_SVG,
        PAUSE => PAUSE_SVG,
        PREV => PREV_SVG,
        NEXT => NEXT_SVG,
        KEYFRAME => KEYFRAME_SVG,
        FILM => FILM_SVG,
        WARNING => WARNING_SVG,
        CHECK => CHECK_SVG,
        CHEVRON_DOWN => CHEVRON_DOWN_SVG,
        SETTINGS => SETTINGS_SVG,
        CLOSE => CLOSE_SVG,
        _ => return None,
    })
}

pub fn icon(path: &'static str, size: f32, color: u32) -> Svg {
    svg().path(path).size(px(size)).text_color(rgb(color))
}
