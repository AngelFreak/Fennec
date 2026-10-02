//! Export: the Nørregade 14 inspection as DOCX (the mockup's default), TXT
//! and PDF, with "Udarbejdet af" still empty.

use fennec::export::Format;
use fennec::ui::Nav;

use crate::Scene;

pub fn capture(s: &Scene) {
    s.w.sidebar.go(Nav::Dictate);
    s.w.dictation.open_document(s.besigtigelse).unwrap();
    s.w.dictation.editor.set_preview(None);
    s.w.export_current();
    for (format, shot) in [
        (Format::Docx, "export-docx"),
        (Format::Txt, "export-txt"),
        (Format::Pdf, "export-pdf"),
    ] {
        s.w.export.set_format(format);
        s.shot(shot);
    }
    s.w.close_sub_page();
}
