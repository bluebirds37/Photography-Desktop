//! Adobe XMP sidecar export, compatible with Camera Raw.
//!
//! The standard Camera Raw numbers (`crs:*`) are written without touching
//! unrelated metadata already present in an existing sidecar. Our own workflow
//! Workflow markers are written into the project's dedicated `lrt:` namespace.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::core::model::{ImageEntry, Sequence};

fn f(v: f32) -> String {
    let s = format!("{:.4}", v);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn full_document(im: &ImageEntry) -> String {
    let total_ev = im.adj.total_ev();
    let (wb, temp, tint) = if im.adj.temperature.abs() > 0.01 || im.adj.tint.abs() > 0.01 {
        ("Custom", 5500.0 + im.adj.temperature * 20.0, im.adj.tint)
    } else {
        ("As Shot", 0.0, 0.0)
    };
    let mut crs = String::new();
    crs.push_str(&format!(
        r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Timelapse Assistant">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about=""
        xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
        xmlns:lrt="urn:timelapse-assistant:workflow:1.0"
        xmlns:tiff="http://ns.adobe.com/tiff/1.0/">
      <crs:Version>15.0</crs:Version>
      <crs:ProcessVersion>15.4</crs:ProcessVersion>
      <crs:HasSettings>true</crs:HasSettings>
      <crs:WhiteBalance>{wb}</crs:WhiteBalance>
"#
    ));
    if wb == "Custom" {
        crs.push_str(&format!(
            "      <crs:Temperature>{}</crs:Temperature>\n      <crs:Tint>{}</crs:Tint>\n",
            f(temp),
            f(tint)
        ));
    }
    crs.push_str(&format!(
        "      <crs:Exposure2012>{}</crs:Exposure2012>\n\
         \x20     <crs:Contrast2012>{}</crs:Contrast2012>\n\
         \x20     <crs:Highlights2012>{}</crs:Highlights2012>\n\
         \x20     <crs:Shadows2012>{}</crs:Shadows2012>\n\
         \x20     <crs:Whites2012>{}</crs:Whites2012>\n\
         \x20     <crs:Blacks2012>{}</crs:Blacks2012>\n\
         \x20     <crs:Saturation>{}</crs:Saturation>\n\
         \x20     <crs:Vibrance>{}</crs:Vibrance>\n\
         \x20     <crs:HasCrop>false</crs:HasCrop>\n\
         \x20     <lrt:Keyframe>{}</lrt:Keyframe>\n\
         \x20     <lrt:Deflicker>{}</lrt:Deflicker>\n\
         \x20     <lrt:Exposure2012>{}</lrt:Exposure2012>\n\
         \x20   </rdf:Description>\n  </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>",
        f(total_ev),
        f(im.adj.contrast),
        f(im.adj.highlights),
        f(im.adj.shadows),
        f(im.adj.whites),
        f(im.adj.blacks),
        f(im.adj.saturation),
        f(im.adj.vibrance),
        if im.keyframe { "True" } else { "False" },
        f(im.adj.deflicker),
        f(im.adj.exposure)
    ));
    crs
}

/// Update a tag in an existing XMP document, inserting it before the closing
/// description if needed. Returns false when the document has no description.
fn set_tag(xml: &mut String, tag: &str, value: &str) -> bool {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    if let Some(start) = xml.find(&open) {
        if let Some(end_rel) = xml[start..].find(&close) {
            let end = start + end_rel + close.len();
            xml.replace_range(start..end, &format!("{open}{value}{close}"));
            return true;
        }
    }
    if let Some(pos) = xml.find("</rdf:Description>") {
        xml.insert_str(pos, &format!("      {open}{value}{close}\n    "));
        return true;
    }
    false
}

fn ensure_namespaces(xml: &mut String) -> bool {
    let Some(desc) = xml.find("<rdf:Description") else {
        return false;
    };
    let Some(end_rel) = xml[desc..].find('>') else {
        return false;
    };
    let end = desc + end_rel;
    let header = &xml[desc..end];
    let mut insert = String::new();
    if !header.contains("xmlns:crs") {
        insert.push_str(" xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"");
    }
    if !header.contains("xmlns:lrt") {
        insert.push_str(" xmlns:lrt=\"urn:timelapse-assistant:workflow:1.0\"");
    }
    if !insert.is_empty() {
        xml.insert_str(end, &insert);
    }
    true
}

pub fn sidecar_path(folder: &Path, im: &ImageEntry) -> PathBuf {
    folder
        .join(
            Path::new(&im.file_name)
                .file_stem()
                .map(|s| s.to_owned())
                .unwrap_or_default(),
        )
        .with_extension("xmp")
}

pub fn write_sidecar(folder: &Path, im: &ImageEntry) -> Result<PathBuf> {
    let path = sidecar_path(folder, im);
    let total_ev = im.adj.total_ev();
    let (wb, temp, tint) = if im.adj.temperature.abs() > 0.01 || im.adj.tint.abs() > 0.01 {
        ("Custom", 5500.0 + im.adj.temperature * 20.0, im.adj.tint)
    } else {
        ("As Shot", 0.0, 0.0)
    };

    let mut xml = match std::fs::read_to_string(&path) {
        Ok(existing) if existing.contains("<rdf:Description") => existing,
        _ => full_document(im),
    };
    ensure_namespaces(&mut xml);
    if !xml.contains("<crs:HasSettings>") {
        set_tag(&mut xml, "crs:HasSettings", "true");
    }
    set_tag(&mut xml, "crs:WhiteBalance", wb);
    if wb == "Custom" {
        set_tag(&mut xml, "crs:Temperature", &format!("{temp:.0}"));
        set_tag(&mut xml, "crs:Tint", &f(tint));
    }
    set_tag(&mut xml, "crs:Exposure2012", &f(total_ev));
    set_tag(&mut xml, "crs:Contrast2012", &f(im.adj.contrast));
    set_tag(&mut xml, "crs:Highlights2012", &f(im.adj.highlights));
    set_tag(&mut xml, "crs:Shadows2012", &f(im.adj.shadows));
    set_tag(&mut xml, "crs:Whites2012", &f(im.adj.whites));
    set_tag(&mut xml, "crs:Blacks2012", &f(im.adj.blacks));
    set_tag(&mut xml, "crs:Saturation", &f(im.adj.saturation));
    set_tag(&mut xml, "crs:Vibrance", &f(im.adj.vibrance));
    set_tag(
        &mut xml,
        "lrt:Keyframe",
        if im.keyframe { "True" } else { "False" },
    );
    set_tag(&mut xml, "lrt:Deflicker", &f(im.adj.deflicker));
    set_tag(&mut xml, "lrt:Exposure2012", &f(im.adj.exposure));
    if !xml.contains("<crs:Exposure2012>") {
        // The existing sidecar may be self-closing or otherwise unwritable;
        // fall back to a complete document rather than silently losing edits.
        xml = full_document(im);
    }

    let tmp = path.with_extension("xmp.tmp");
    std::fs::write(&tmp, xml).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

pub fn export_all(seq: &Sequence) -> Result<usize> {
    let mut n = 0;
    for im in &seq.images {
        if im.skipped {
            continue;
        }
        write_sidecar(&seq.folder, im)?;
        n += 1;
    }
    Ok(n)
}

/// Update workflow-related tags inside an XMP file generated by this app.
pub fn write_workflow_tags(xml: &str, keyframe: bool, exposure: f32, deflicker: f32) -> String {
    let mut out = xml.to_string();
    set_tag(
        &mut out,
        "lrt:Keyframe",
        if keyframe { "True" } else { "False" },
    );
    set_tag(&mut out, "crs:Exposure2012", &f(exposure + deflicker));
    set_tag(&mut out, "lrt:Deflicker", &f(deflicker));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::Adjustments;

    fn image(temp: f32) -> ImageEntry {
        ImageEntry {
            file_name: "IMG_0001.CR3".into(),
            adj: Adjustments {
                exposure: 0.5,
                deflicker: -0.1,
                temperature: temp,
                ..Adjustments::default()
            },
            keyframe: true,
            ..ImageEntry::default()
        }
    }

    #[test]
    fn creates_sidecar_with_custom_wb() {
        let im = image(10.0);
        let xml = full_document(&im);
        assert!(xml.contains("<crs:Exposure2012>0.4</crs:Exposure2012>"));
        assert!(xml.contains("<crs:Temperature>5700</crs:Temperature>"));
        assert!(xml.contains("<lrt:Keyframe>True</lrt:Keyframe>"));
    }

    #[test]
    fn updates_existing_document_without_losing_tags() {
        let mut xml = r#"<x:xmpmeta><rdf:RDF><rdf:Description xmlns:crs="x"><crs:Rating>5</crs:Rating></rdf:Description></rdf:RDF></x:xmpmeta>"#.to_string();
        ensure_namespaces(&mut xml);
        assert!(set_tag(&mut xml, "crs:Exposure2012", "1.25"));
        assert!(xml.contains("<crs:Rating>5</crs:Rating>"));
        assert!(xml.contains("<crs:Exposure2012>1.25</crs:Exposure2012>"));
    }

    #[test]
    fn sidecar_name_replaces_raw_extension() {
        let im = image(0.0);
        assert_eq!(
            sidecar_path(Path::new("C:/seq"), &im),
            PathBuf::from("C:/seq/IMG_0001.xmp")
        );
    }
}
