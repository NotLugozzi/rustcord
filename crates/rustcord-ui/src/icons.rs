#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Icon {
    Phone,
    PhoneMissed,
    PhoneCall,
    PhoneOff,
    UserPlus,
    UserMinus,
    Pencil,
    Image,
    Pin,
    CircleHelp,
    Volume,
    VolumeOff,
    Video,
    Users,
    Search,
    Mic,
    MicOff,
    Headphones,
    HeadphoneOff,
    Play,
    Pause,
    Replay,
    Maximize,
    Close,
    MessageCircle,
    MonitorUp,
    AudioWaveform,
    Info,
    Film,
    Music,
    Package,
    FileText,
    Folder,
    Settings,
    CirclePlus,
    Gif,
    Sticker,
    Smile,
    ChevronDown,
    UserRound,
}

impl Icon {
    pub const ALL: [Icon; 40] = [
        Icon::Phone,
        Icon::PhoneMissed,
        Icon::PhoneCall,
        Icon::PhoneOff,
        Icon::UserPlus,
        Icon::UserMinus,
        Icon::Pencil,
        Icon::Image,
        Icon::Pin,
        Icon::CircleHelp,
        Icon::Volume,
        Icon::VolumeOff,
        Icon::Video,
        Icon::Users,
        Icon::Search,
        Icon::Mic,
        Icon::MicOff,
        Icon::Headphones,
        Icon::HeadphoneOff,
        Icon::Play,
        Icon::Pause,
        Icon::Replay,
        Icon::Maximize,
        Icon::Close,
        Icon::MessageCircle,
        Icon::MonitorUp,
        Icon::AudioWaveform,
        Icon::Info,
        Icon::Film,
        Icon::Music,
        Icon::Package,
        Icon::FileText,
        Icon::Folder,
        Icon::Settings,
        Icon::CirclePlus,
        Icon::Gif,
        Icon::Sticker,
        Icon::Smile,
        Icon::ChevronDown,
        Icon::UserRound,
    ];

    fn svg(self) -> &'static str {
        match self {
            Icon::Phone => include_str!("../assets/icons/phone.svg"),
            Icon::PhoneMissed => include_str!("../assets/icons/phone-missed.svg"),
            Icon::PhoneCall => include_str!("../assets/icons/phone-call.svg"),
            Icon::PhoneOff => include_str!("../assets/icons/phone-off.svg"),
            Icon::UserPlus => include_str!("../assets/icons/user-plus.svg"),
            Icon::UserMinus => include_str!("../assets/icons/user-minus.svg"),
            Icon::Pencil => include_str!("../assets/icons/pencil.svg"),
            Icon::Image => include_str!("../assets/icons/image.svg"),
            Icon::Pin => include_str!("../assets/icons/pin.svg"),
            Icon::CircleHelp => include_str!("../assets/icons/circle-help.svg"),
            Icon::Volume => include_str!("../assets/icons/volume-2.svg"),
            Icon::VolumeOff => include_str!("../assets/icons/volume-x.svg"),
            Icon::Video => include_str!("../assets/icons/video.svg"),
            Icon::Users => include_str!("../assets/icons/users.svg"),
            Icon::Search => include_str!("../assets/icons/search.svg"),
            Icon::Mic => include_str!("../assets/icons/mic.svg"),
            Icon::MicOff => include_str!("../assets/icons/mic-off.svg"),
            Icon::Headphones => include_str!("../assets/icons/headphones.svg"),
            Icon::HeadphoneOff => include_str!("../assets/icons/headphone-off.svg"),
            Icon::Play => include_str!("../assets/icons/play.svg"),
            Icon::Pause => include_str!("../assets/icons/pause.svg"),
            Icon::Replay => include_str!("../assets/icons/rotate-ccw.svg"),
            Icon::Maximize => include_str!("../assets/icons/maximize.svg"),
            Icon::Close => include_str!("../assets/icons/x.svg"),
            Icon::MessageCircle => include_str!("../assets/icons/message-circle.svg"),
            Icon::MonitorUp => include_str!("../assets/icons/monitor-up.svg"),
            Icon::AudioWaveform => include_str!("../assets/icons/audio-waveform.svg"),
            Icon::Info => include_str!("../assets/icons/info.svg"),
            Icon::Film => include_str!("../assets/icons/film.svg"),
            Icon::Music => include_str!("../assets/icons/music.svg"),
            Icon::Package => include_str!("../assets/icons/package.svg"),
            Icon::FileText => include_str!("../assets/icons/file-text.svg"),
            Icon::Folder => include_str!("../assets/icons/folder.svg"),
            Icon::Settings => include_str!("../assets/icons/settings.svg"),
            Icon::CirclePlus => include_str!("../assets/icons/circle-plus.svg"),
            Icon::Gif => include_str!("../assets/icons/gif.svg"),
            Icon::Sticker => include_str!("../assets/icons/sticker.svg"),
            Icon::Smile => include_str!("../assets/icons/smile.svg"),
            Icon::ChevronDown => include_str!("../assets/icons/chevron-down.svg"),
            Icon::UserRound => include_str!("../assets/icons/user-round.svg"),
        }
    }
}

pub fn rasterize(icon: Icon, px: u32) -> Option<image::RgbaImage> {
    let svg = icon.svg().replace("currentColor", "#ffffff");
    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default())
        .inspect_err(|e| tracing::warn!("icon {icon:?} failed to parse: {e}"))
        .ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(px, px)?;
    let size = tree.size();
    let transform = resvg::tiny_skia::Transform::from_scale(px as f32 / size.width(), px as f32 / size.height());
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let mut rgba = image::RgbaImage::from_raw(px, px, pixmap.take())?;
    for pixel in rgba.pixels_mut() {
        pixel.0 = [255, 255, 255, pixel.0[3]];
    }
    Some(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_rasterizes_to_visible_pixels() {
        for icon in Icon::ALL {
            let rgba = rasterize(icon, 24).unwrap_or_else(|| panic!("{icon:?} did not rasterize"));
            assert!(rgba.pixels().any(|p| p.0[3] > 0), "{icon:?} is blank");
        }
    }

    #[test]
    fn icons_are_white_masks_so_they_can_be_tinted() {
        let rgba = rasterize(Icon::Phone, 32).unwrap();
        assert!(rgba.pixels().all(|p| p.0[..3] == [255, 255, 255]));
    }
}
