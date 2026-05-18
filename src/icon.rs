use std::collections::HashMap;
use std::convert::TryFrom;

use gtk4::gdk_pixbuf::{Colorspace, InterpType, Pixbuf, PixbufLoader};
use gtk4::prelude::*;
use gtk4::{glib, Image};
use zbus::zvariant::{OwnedValue, Structure};

use crate::history::HistoryEntry;
use crate::notification::Notification;

pub fn build_icon(notif: &Notification, history_entry: &mut HistoryEntry) -> Image {
    if let Some(img) = icon_from_hints(&notif.hints, history_entry) {
        return img;
    }

    if let Some(img) = icon_from_path(&notif.app_icon) {
        return img;
    }

    const ICON_SIZE: i32 = 64;
    let img = Image::builder()
        .icon_name(&notif.app_icon)
        .pixel_size(ICON_SIZE)
        .css_classes(vec!["notification-icon"])
        .build();

    img
}

pub fn icon_from_hints(
    hints: &HashMap<String, OwnedValue>,
    history_entry: &mut HistoryEntry,
) -> Option<Image> {
    if let Some(value) = hints.get("image-path") {
        if let Some(img) = icon_from_hint_value(value) {
            history_entry.app_icon = String::try_from(value.try_to_owned().unwrap()).unwrap();

            return Some(img);
        }
    }

    if let Some(value) = hints.get("image-data") {
        if let Some(img) = icon_from_hint_value(value) {
            return Some(img);
        }
    }

    for (key, value) in hints {
        if !hint_key_might_be_icon(key) {
            continue;
        }
        if let Some(img) = icon_from_hint_value(value) {
            return Some(img);
        }
    }

    None
}

pub fn hint_key_might_be_icon(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("image") || key.contains("icon") || key.contains("app_icon")
}

pub fn may_have_icon_from_hint(hints: &HashMap<String, OwnedValue>) -> bool {
    if hints.get("image-path").is_some() {
        return true;
    }

    if hints.get("image-data").is_some() {
        return true;
    }

    for (key, _) in hints {
        if !hint_key_might_be_icon(key) {
            continue;
        }

        return true;
    }

    false
}

pub fn icon_from_hint_value(value: &OwnedValue) -> Option<Image> {
    if let Ok(owned_value) = value.try_clone() {
        if let Ok(path) = String::try_from(owned_value) {
            if let Some(img) = icon_from_path(&path) {
                return Some(img);
            }
            if !path.is_empty() {
                let img = Image::builder()
                    .icon_name(&path)
                    .pixel_size(64)
                    .css_classes(vec!["notification-icon"])
                    .build();
                return Some(img);
            }
        }
    }

    if let Ok(owned_value) = value.try_clone() {
        if let Ok(bytes) = Vec::<u8>::try_from(owned_value) {
            if let Some(img) = image_from_raw_bytes(&bytes) {
                return Some(img);
            }
        }
    }

    if let Ok(owned_value) = value.try_clone() {
        if let Ok(structure) = Structure::try_from(owned_value) {
            if let Some(img) = image_from_image_data_structure(structure) {
                return Some(img);
            }
        }
    }

    None
}

pub fn image_from_image_data_structure(structure: Structure) -> Option<Image> {
    if let Ok((width, height, rowstride, has_alpha, bits, channels, data)) =
        <(i32, i32, i32, bool, i32, i32, Vec<u8>)>::try_from(structure)
    {
        if width <= 0 || height <= 0 || bits != 8 {
            return None;
        }

        let channels = channels.max(1).min(4);
        let min_stride = width * channels;
        let rowstride = if rowstride <= 0 || rowstride < min_stride {
            min_stride
        } else {
            rowstride
        };

        let bytes = glib::Bytes::from(&data[..]);
        let pixbuf = Pixbuf::from_bytes(
            &bytes,
            Colorspace::Rgb,
            has_alpha,
            bits,
            width,
            height,
            rowstride,
        );

        if pixbuf.width() == 0 || pixbuf.height() == 0 {
            return None;
        }

        let max_icon_size = 64i32;
        let scale_factor = (width.max(height) as f64) / (max_icon_size as f64);
        let scale_factor = scale_factor.max(1.0); // Only scale down if larger
        let desired_w = (width as f64 / scale_factor).round() as i32;
        let desired_h = (height as f64 / scale_factor).round() as i32;
        let img_pix =
            if let Some(pb) = pixbuf.scale_simple(desired_w, desired_h, InterpType::Bilinear) {
                pb
            } else {
                pixbuf
            };
        let img = Image::from_pixbuf(Some(&img_pix));
        img.set_pixel_size(img_pix.width());
        img.add_css_class("notification-icon");
        return Some(img);
    }

    None
}

pub fn image_from_raw_bytes(bytes: &[u8]) -> Option<Image> {
    let loader = PixbufLoader::new();
    if loader.write_bytes(&glib::Bytes::from(bytes)).is_ok() && loader.close().is_ok() {
        if let Some(pb) = loader.pixbuf() {
            let w = pb.width() as i32;
            let h = pb.height() as i32;
            let max_icon_size = 64i32;
            let scale_factor = (w.max(h) as f64) / (max_icon_size as f64);
            let scale_factor = scale_factor.max(1.0); // Only scale down if larger
            let desired_w = (w as f64 / scale_factor).round() as i32;
            let desired_h = (h as f64 / scale_factor).round() as i32;
            let final_pb =
                if let Some(s) = pb.scale_simple(desired_w, desired_h, InterpType::Bilinear) {
                    s
                } else {
                    pb
                };
            let img = Image::from_pixbuf(Some(&final_pb));
            img.set_pixel_size(final_pb.width());
            img.add_css_class("notification-icon");
            return Some(img);
        }
    }

    None
}

pub fn icon_from_path(name_or_path: &str) -> Option<Image> {
    const ICON_FALLBACK: i32 = 64;

    if name_or_path.is_empty() {
        return None;
    }

    let maybe_path = if let Some(stripped) = name_or_path.strip_prefix("file://") {
        stripped.strip_prefix("localhost/").unwrap_or(stripped)
    } else {
        name_or_path
    };

    // Local file path: load full image and scale proportionally but cap to ICON_FALLBACK
    if std::path::Path::new(maybe_path).exists() {
        if let Ok(pb_full) = Pixbuf::from_file(maybe_path) {
            let (w, h) = (pb_full.width() as i32, pb_full.height() as i32);
            let max_icon_size = ICON_FALLBACK;
            let scale_factor = (w.max(h) as f64) / (max_icon_size as f64);
            let scale_factor = scale_factor.max(1.0); // only scale down
            let desired_w = (w as f64 / scale_factor).round() as i32;
            let desired_h = (h as f64 / scale_factor).round() as i32;
            if let Some(pb) = pb_full.scale_simple(desired_w, desired_h, InterpType::Bilinear) {
                let img = Image::from_pixbuf(Some(&pb));
                img.set_pixel_size(max_icon_size);
                img.add_css_class("notification-icon");
                return Some(img);
            }
            let img = Image::from_pixbuf(Some(&pb_full));
            img.set_pixel_size(ICON_FALLBACK);
            img.add_css_class("notification-icon");
            return Some(img);
        }
    }

    // Remote HTTP(S) URL: fetch asynchronously and update image when ready
    if name_or_path.starts_with("http://") || name_or_path.starts_with("https://") {
        // placeholder image while fetching
        let img = Image::builder()
            .icon_name("image-x-generic")
            .pixel_size(ICON_FALLBACK)
            .css_classes(vec!["notification-icon"])
            .build();

        let url = name_or_path.to_string();
        let (sender, receiver) = std::sync::mpsc::channel::<Vec<u8>>();
        let img_clone = img.clone();
        glib::idle_add_local(move || {
            use std::sync::mpsc::TryRecvError;
            match receiver.try_recv() {
                Ok(vec) => {
                    let loader = PixbufLoader::new();
                    let _ = loader.write_bytes(&glib::Bytes::from(&vec[..]));
                    let _ = loader.close();
                    if let Some(pb) = loader.pixbuf() {
                        let final_pb = if let Some(s) =
                            pb.scale_simple(ICON_FALLBACK, ICON_FALLBACK, InterpType::Bilinear)
                        {
                            s
                        } else {
                            pb
                        };
                        img_clone.set_from_pixbuf(Some(&final_pb));
                        img_clone.set_pixel_size(ICON_FALLBACK);
                        img_clone.add_css_class("notification-icon");
                    }
                    glib::ControlFlow::Break
                }
                Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });

        std::thread::spawn(move || {
            if let Ok(resp) = reqwest::blocking::get(&url) {
                if let Ok(bytes) = resp.bytes() {
                    let vec = bytes.to_vec();
                    let _ = sender.send(vec);
                }
            }
        });

        return Some(img);
    }

    let img = Image::builder()
        .icon_name(name_or_path)
        .pixel_size(ICON_FALLBACK)
        .css_classes(vec!["notification-icon"])
        .build();
    Some(img)
}
