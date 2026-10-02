//! A4 PDF via cairo + pango: real text layout, system fonts, Danish text.
//! Rendered twice: once to count pages (for "Side N af M"), once to draw.

use std::path::Path;

use cairo::{Context, PdfSurface};
use pango::{AttrColor, AttrList, FontDescription, Layout, WrapMode};

use super::{ExportError, Report, ReportParagraph};

const PAGE_W: f64 = 595.28;
const PAGE_H: f64 = 841.89;
const MARGIN: f64 = 64.0;
const FOOTER_SPACE: f64 = 36.0;
const LABEL_W: f64 = 140.0;

fn load_logo(report: &Report) -> Result<Option<cairo::ImageSurface>, ExportError> {
    Ok(match &report.logo {
        Some(p) => {
            let mut f = std::fs::File::open(p).map_err(|source| ExportError::Io {
                path: p.clone(),
                source,
            })?;
            Some(
                cairo::ImageSurface::create_from_png(&mut f)
                    .map_err(|e| pdf_err(format!("logo {}: {e}", p.display())))?,
            )
        }
        None => None,
    })
}

/// The first page as an image `width_px` wide, white background, for previews.
pub fn preview(report: &Report, width_px: i32) -> Result<cairo::ImageSurface, ExportError> {
    let logo = load_logo(report)?;
    let scale = f64::from(width_px) / PAGE_W;
    let height_px = (PAGE_H * scale).round() as i32;
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width_px, height_px).map_err(pdf_err)?;
    {
        let cr = Context::new(&surface).map_err(pdf_err)?;
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.paint().map_err(pdf_err)?;
        cr.scale(scale, scale);
        match render(report, &cr, logo.as_ref(), None, true) {
            Ok(_) => {}
            Err(ExportError::Pdf(m)) if m == PREVIEW_DONE => {}
            Err(e) => return Err(e),
        }
    }
    surface.flush();
    Ok(surface)
}

pub fn write(report: &Report, path: &Path) -> Result<(), ExportError> {
    let logo = load_logo(report)?;
    let pages = {
        let surface = PdfSurface::for_stream(PAGE_W, PAGE_H, std::io::sink()).map_err(pdf_err)?;
        render(
            report,
            &Context::new(&surface).map_err(pdf_err)?,
            logo.as_ref(),
            None,
            false,
        )?
    };
    let surface = PdfSurface::new(PAGE_W, PAGE_H, path).map_err(pdf_err)?;
    render(
        report,
        &Context::new(&surface).map_err(pdf_err)?,
        logo.as_ref(),
        Some(pages),
        false,
    )?;
    surface.finish();
    Ok(())
}

fn pdf_err(e: impl std::fmt::Display) -> ExportError {
    ExportError::Pdf(e.to_string())
}

struct Pager<'a> {
    cr: &'a Context,
    report: &'a Report,
    y: f64,
    page: usize,
    total: Option<usize>,
    /// Preview renders only the first page.
    first_page_only: bool,
}

/// Internal signal that a first-page preview is complete.
const PREVIEW_DONE: &str = "\u{0}preview-done";

impl Pager<'_> {
    fn bottom(&self) -> f64 {
        PAGE_H - MARGIN - FOOTER_SPACE
    }

    fn new_page(&mut self) -> Result<(), ExportError> {
        self.footer()?;
        if self.first_page_only {
            return Err(ExportError::Pdf(PREVIEW_DONE.into()));
        }
        self.cr.show_page().map_err(pdf_err)?;
        self.page += 1;
        self.y = MARGIN;
        Ok(())
    }

    fn footer(&self) -> Result<(), ExportError> {
        let y = PAGE_H - MARGIN;
        let small = font(&self.report.body_font, 8.0, false);
        if !self.report.footer.is_empty() {
            let l = layout(self.cr, &self.report.footer, &small, PAGE_W - 2.0 * MARGIN - 80.0);
            gray(self.cr);
            self.cr.move_to(MARGIN, y);
            pangocairo::functions::show_layout(self.cr, &l);
        }
        let pages = match self.total {
            Some(t) => format!("Side {} af {t}", self.page),
            None => format!("Side {}", self.page),
        };
        let l = layout(self.cr, &pages, &small, 80.0);
        l.set_alignment(pango::Alignment::Right);
        gray(self.cr);
        self.cr.move_to(PAGE_W - MARGIN - 80.0, y);
        pangocairo::functions::show_layout(self.cr, &l);
        black(self.cr);
        Ok(())
    }

    /// Places a layout line by line, breaking pages between lines.
    fn place(&mut self, l: &Layout, x: f64) -> Result<(), ExportError> {
        for line in l.lines_readonly() {
            let (_, logical) = line.extents();
            let h = f64::from(logical.height()) / f64::from(pango::SCALE);
            let ascent = -f64::from(logical.y()) / f64::from(pango::SCALE);
            if self.y + h > self.bottom() {
                self.new_page()?;
            }
            self.cr.move_to(x, self.y + ascent);
            pangocairo::functions::show_layout_line(self.cr, &line);
            self.y += h;
        }
        Ok(())
    }

    fn gap(&mut self, h: f64) {
        self.y += h;
    }

    /// Starts a new page unless at least `h` points remain.
    fn keep(&mut self, h: f64) -> Result<(), ExportError> {
        if self.y + h > self.bottom() {
            self.new_page()
        } else {
            Ok(())
        }
    }
}

fn render(
    report: &Report,
    cr: &Context,
    logo: Option<&cairo::ImageSurface>,
    total: Option<usize>,
    first_page_only: bool,
) -> Result<usize, ExportError> {
    let width = PAGE_W - 2.0 * MARGIN;
    let body = font(&report.body_font, report.body_size_pt, false);
    let bold = |size: f64| font(&report.body_font, size, true);
    let mut p = Pager {
        first_page_only,
        cr,
        report,
        y: MARGIN,
        page: 1,
        total,
    };
    black(cr);

    if let Some(title) = &report.collection_title {
        p.place(&layout(cr, title, &bold(20.0), width), MARGIN)?;
        p.gap(12.0);
        if report.table_of_contents {
            for (i, s) in report.sections.iter().enumerate() {
                p.place(
                    &layout(cr, &format!("{}. {} ({})", i + 1, s.title, s.date), &body, width),
                    MARGIN,
                )?;
            }
        }
    }

    for (i, section) in report.sections.iter().enumerate() {
        if i > 0 || report.collection_title.is_some() {
            p.new_page()?;
        }
        if let Some(img) = logo.filter(|_| i == 0 || report.collection_title.is_some()) {
            let h = 34.0;
            let scale = h / f64::from(img.height().max(1));
            cr.save().map_err(pdf_err)?;
            cr.translate(MARGIN, p.y);
            cr.scale(scale, scale);
            cr.set_source_surface(img, 0.0, 0.0).map_err(pdf_err)?;
            cr.paint().map_err(pdf_err)?;
            cr.restore().map_err(pdf_err)?;
            if report.heading.is_empty() {
                p.gap(h + 12.0);
            }
        }
        if !report.heading.is_empty() {
            let l = layout(cr, &report.heading, &bold(11.0), width);
            // No letter-spacing: it makes PDF text extraction split the word into letters.
            l.set_alignment(pango::Alignment::Right);
            if logo.is_some() && (i == 0 || report.collection_title.is_some()) {
                p.gap(12.0);
            }
            p.place(&l, MARGIN)?;
            p.gap(6.0);
            cr.set_line_width(1.5);
            cr.move_to(MARGIN, p.y);
            cr.line_to(PAGE_W - MARGIN, p.y);
            cr.stroke().map_err(pdf_err)?;
            p.gap(14.0);
        }
        let label_font = font(&report.body_font, 9.0, false);
        for (label, value) in &section.fields {
            let lv = layout(cr, value, &body, width - LABEL_W);
            let ll = layout(cr, label, &label_font, LABEL_W - 8.0);
            let h = f64::from(lv.pixel_size().1).max(f64::from(ll.pixel_size().1));
            p.keep(h)?;
            gray(cr);
            cr.move_to(MARGIN, p.y + 1.5);
            pangocairo::functions::show_layout(cr, &ll);
            black(cr);
            cr.move_to(MARGIN + LABEL_W, p.y);
            pangocairo::functions::show_layout(cr, &lv);
            p.gap(h + 4.0);
        }
        if !section.fields.is_empty() {
            p.gap(10.0);
        }
        p.keep(60.0)?;
        p.place(&layout(cr, &section.title, &bold(17.0), width), MARGIN)?;
        p.gap(10.0);
        if let Some(summary) = &section.summary {
            p.keep(40.0)?;
            p.place(&layout(cr, &report.summary_heading, &bold(12.0), width), MARGIN)?;
            p.gap(4.0);
            p.place(&layout(cr, summary.trim(), &body, width), MARGIN)?;
            p.gap(10.0);
        }
        for para in &section.paragraphs {
            p.place(&paragraph_layout(cr, para, &body, width), MARGIN)?;
            p.gap(report.body_size_pt * 0.7);
        }
    }
    p.footer()?;
    if !first_page_only {
        cr.show_page().map_err(pdf_err)?;
    }
    Ok(p.page)
}

fn paragraph_layout(cr: &Context, para: &ReportParagraph, f: &FontDescription, width: f64) -> Layout {
    let prefix = para
        .timestamp
        .as_ref()
        .map(|t| format!("[{t}] "))
        .unwrap_or_default();
    let l = layout(cr, &format!("{prefix}{}", para.text), f, width);
    let attrs = AttrList::new();
    if !prefix.is_empty() {
        let mut c = AttrColor::new_foreground(0x5A5A, 0x6161, 0x7070);
        c.set_start_index(0);
        c.set_end_index(prefix.len() as u32);
        attrs.insert(c);
    }
    for r in &para.highlight {
        let mut bg = AttrColor::new_background(0xFFFF, 0xF0F0, 0x8080);
        bg.set_start_index((prefix.len() + r.start) as u32);
        bg.set_end_index((prefix.len() + r.end) as u32);
        attrs.insert(bg);
    }
    l.set_attributes(Some(&attrs));
    l
}

fn layout(cr: &Context, text: &str, f: &FontDescription, width: f64) -> Layout {
    let l = pangocairo::functions::create_layout(cr);
    l.set_font_description(Some(f));
    l.set_width((width * f64::from(pango::SCALE)) as i32);
    l.set_wrap(WrapMode::WordChar);
    l.set_text(text);
    l
}

fn font(family: &str, size: f64, bold: bool) -> FontDescription {
    let mut f = FontDescription::new();
    // Fallback families keep output readable if the template's font is missing.
    f.set_family(&format!("{family},Source Serif 4,Noto Serif,DejaVu Serif,Serif"));
    f.set_size((size * f64::from(pango::SCALE)) as i32);
    if bold {
        f.set_weight(pango::Weight::Bold);
    }
    f
}

fn gray(cr: &Context) {
    cr.set_source_rgb(0.353, 0.380, 0.439);
}

fn black(cr: &Context) {
    cr.set_source_rgb(0.082, 0.090, 0.110);
}
