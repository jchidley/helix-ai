use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use helix_core::{Selection, Transaction};
use helix_view::editor::Action;
use helix_view::tree::{Dimension, Resize};
use helix_view::{DocumentId, Editor, ViewId};

use super::config::AiConfig;

const DEFAULT_INPUT_HEIGHT: u16 = 10;

#[derive(Debug, Clone)]
pub struct AiBuffers {
    pub output_doc: DocumentId,
    pub input_doc: DocumentId,
    pub output_path: PathBuf,
    pub input_path: PathBuf,
}

pub fn ensure_buffers(editor: &mut Editor, config: &AiConfig) -> Result<AiBuffers> {
    let output_path = buffer_path(&config.general.output_buffer);
    let input_path = buffer_path(&config.general.input_buffer);

    let (output_doc, _output_created) = ensure_buffer(editor, &output_path, Action::Replace)?;
    set_readonly(editor, output_doc, true);
    set_markdown_language(editor, output_doc);

    if let Some(view_id) = find_view_for_doc(editor, output_doc) {
        editor.focus(view_id);
    }

    let (input_doc, input_created) = ensure_buffer(editor, &input_path, Action::HorizontalSplit)?;
    set_readonly(editor, input_doc, false);
    set_markdown_language(editor, input_doc);

    if let Some(view_id) = find_view_for_doc(editor, input_doc) {
        if input_created {
            resize_to_height(editor, view_id, DEFAULT_INPUT_HEIGHT);
        }
        editor.focus(view_id);
    }

    Ok(AiBuffers {
        output_doc,
        input_doc,
        output_path,
        input_path,
    })
}

pub fn append_output(editor: &mut Editor, output_doc: DocumentId, text: &str) {
    append_output_inner(editor, output_doc, text, true);
}

pub fn append_output_raw(editor: &mut Editor, output_doc: DocumentId, text: &str) {
    append_output_inner(editor, output_doc, text, false);
}

fn append_output_inner(editor: &mut Editor, output_doc: DocumentId, text: &str, add_newline: bool) {
    let view_id = find_view_for_doc(editor, output_doc).unwrap_or(editor.tree.focus);
    let Some(doc) = editor.documents.get_mut(&output_doc) else {
        return;
    };

    let mut text = text.to_string();
    if add_newline && !text.ends_with('\n') {
        text.push('\n');
    }

    let end = doc.text().len_chars();
    let selection = Selection::point(end);
    let transaction = Transaction::insert(doc.text(), &selection, text.into());
    doc.apply_temporary(&transaction, view_id);
}

pub fn take_prompt(editor: &mut Editor, input_doc: DocumentId) -> Result<String> {
    let view_id = find_view_for_doc(editor, input_doc).unwrap_or(editor.tree.focus);
    let Some(doc) = editor.documents.get_mut(&input_doc) else {
        return Err(anyhow!("ai input buffer is missing"));
    };
    let mut text = doc.text().slice(..).to_string();
    let prompt = text.trim_end().to_string();

    text.clear();
    text.push_str(doc.line_ending.as_str());
    let end = doc.text().len_chars();
    let transaction = Transaction::change(doc.text(), [(0, end, Some(text.into()))].into_iter());
    doc.apply_temporary(&transaction, view_id);

    Ok(prompt)
}

pub fn find_view_for_doc(editor: &Editor, doc_id: DocumentId) -> Option<ViewId> {
    editor
        .tree
        .views()
        .find_map(|(view, _)| if view.doc == doc_id { Some(view.id) } else { None })
}

fn ensure_buffer(editor: &mut Editor, path: &Path, action: Action) -> Result<(DocumentId, bool)> {
    if let Some(doc_id) = editor.document_id_by_path(path) {
        if let Some(view_id) = find_view_for_doc(editor, doc_id) {
            editor.focus(view_id);
        } else {
            editor.switch(doc_id, action);
        }
        return Ok((doc_id, false));
    }

    let doc_id = editor.open(path, action)?;
    Ok((doc_id, true))
}

fn set_readonly(editor: &mut Editor, doc_id: DocumentId, readonly: bool) {
    if let Some(doc) = editor.documents.get_mut(&doc_id) {
        doc.readonly = readonly;
    }
}

fn buffer_path(name: &str) -> PathBuf {
    helix_stdx::path::canonicalize(name)
}

fn resize_to_height(editor: &mut Editor, view_id: ViewId, height: u16) {
    let Some(view) = editor.tree.try_get(view_id) else {
        return;
    };
    let current = view.area.height;
    if current == 0 || current == height {
        return;
    }

    let (resize, delta) = if current > height {
        (Resize::Shrink, current.saturating_sub(height))
    } else {
        (Resize::Grow, height.saturating_sub(current))
    };
    if delta == 0 {
        return;
    }
    editor.resize_buffer(view_id, resize, Dimension::Fixed(delta));
}

fn set_markdown_language(editor: &mut Editor, doc_id: DocumentId) {
    let loader = editor.syn_loader.load();
    let mut set = false;
    if let Some(doc) = editor.documents.get_mut(&doc_id) {
        if doc.set_language_by_language_id("markdown", &loader).is_ok() {
            doc.detect_indent_and_line_ending();
            set = true;
        }
    }

    if !set {
        return;
    }

    editor.refresh_language_servers(doc_id);
    if let Some(doc) = editor.documents.get_mut(&doc_id) {
        let diagnostics = Editor::doc_diagnostics(&editor.language_servers, &editor.diagnostics, doc);
        doc.replace_diagnostics(diagnostics, &[], None);
    }
}
