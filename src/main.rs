use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Id, Key, Layout, Margin,
    Modifiers, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2,
};
use epaint_default_fonts::UBUNTU_LIGHT;
use printpdf::{
    Color as PdfColor, Line as PdfLine, LinePoint, Mm, Op as PdfOp, PaintMode, ParsedFont,
    PdfDocument, PdfFontHandle, PdfPage, PdfSaveOptions, Point as PdfPoint, Polygon, PolygonRing,
    Pt as PdfPt, Rect as PdfRect, Rgb as PdfRgb, TextItem, WindingOrder,
};
use serde::{Deserialize, Serialize};
use std::time::Instant;

// ---------------------------------------------------------------- palette ---

const NAVY: Color32 = Color32::from_rgb(35, 45, 63);
const NAVY_HOVER: Color32 = Color32::from_rgb(52, 65, 87);
const TEXT: Color32 = Color32::from_rgb(43, 48, 58);
const MUTED: Color32 = Color32::from_rgb(122, 130, 143);
const CANVAS: Color32 = Color32::from_rgb(247, 248, 250);
const BORDER: Color32 = Color32::from_rgb(231, 234, 240);
const SOFT: Color32 = Color32::from_rgb(240, 242, 246);
const WHITE: Color32 = Color32::WHITE;
const ACCENT: Color32 = Color32::from_rgb(66, 99, 235);

const BRANCH: [Color32; 5] = [
    Color32::from_rgb(89, 132, 233),
    Color32::from_rgb(57, 184, 138),
    Color32::from_rgb(238, 156, 60),
    Color32::from_rgb(148, 117, 235),
    Color32::from_rgb(235, 119, 151),
];

const V_GAP: f32 = 18.0;
const BRANCH_GAP: f32 = 28.0;

// -------------------------------------------------------------- document ----

#[derive(Clone, Serialize, Deserialize)]
struct Node {
    id: u64,
    text: String,
    #[serde(default)]
    children: Vec<Node>,
}

#[derive(Clone)]
struct EditState {
    id: u64,
    text: String,
    select_all: bool,
}

struct Laid {
    id: u64,
    text: String,
    rect: Rect,
    depth: usize,
    color: usize,
    parent: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Icon {
    Undo,
    Redo,
    Trash,
}

#[derive(Clone, Copy)]
enum Action {
    AddChild,
    NewMap,
    Delete,
    Undo,
    Redo,
    Fit,
    ZoomIn,
    ZoomOut,
    ExportPdf,
}

struct ShotState {
    path: PathBuf,
    frame: u32,
    requested: bool,
}

struct Camera {
    pan: Vec2,
    zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

struct MindMapApp {
    doc: Node,
    next_id: u64,
    selected: u64,
    editing: Option<EditState>,
    undo_stack: Vec<Node>,
    redo_stack: Vec<Node>,
    camera: Camera,
    drag: Option<u64>,
    panning: bool,
    hovered: Option<u64>,
    needs_fit: bool,
    save_ok: bool,
    save_path: PathBuf,
    toast: Option<(String, Instant)>,
    shot: Option<ShotState>,
    auto_pdf: Option<(PathBuf, u32)>,
    pdf_dir: Option<PathBuf>,
}

fn node(id: u64, text: &str, children: Vec<Node>) -> Node {
    Node {
        id,
        text: text.to_owned(),
        children,
    }
}

fn sample_map() -> Node {
    node(
        1,
        "Моя карта",
        vec![
            node(
                2,
                "Идеи",
                vec![
                    node(5, "Новая привычка", vec![]),
                    node(6, "Путешествие", vec![]),
                    node(7, "Свой проект", vec![]),
                ],
            ),
            node(
                3,
                "Планы",
                vec![
                    node(8, "На этой неделе", vec![]),
                    node(9, "Цели на месяц", vec![]),
                ],
            ),
            node(
                4,
                "Ресурсы",
                vec![node(10, "Книги", vec![]), node(11, "Курсы", vec![])],
            ),
            node(
                12,
                "Вопросы",
                vec![
                    node(13, "Что важно сейчас?", vec![]),
                    node(14, "Что можно упростить?", vec![]),
                ],
            ),
        ],
    )
}

fn empty_map() -> Node {
    node(1, "Моя карта", vec![])
}

fn find<'a>(n: &'a Node, id: u64) -> Option<&'a Node> {
    if n.id == id {
        return Some(n);
    }
    n.children.iter().find_map(|c| find(c, id))
}

fn find_mut<'a>(n: &'a mut Node, id: u64) -> Option<&'a mut Node> {
    if n.id == id {
        return Some(n);
    }
    n.children.iter_mut().find_map(|c| find_mut(c, id))
}

fn parent_of(n: &Node, id: u64) -> Option<u64> {
    for c in &n.children {
        if c.id == id {
            return Some(n.id);
        }
        if let Some(p) = parent_of(c, id) {
            return Some(p);
        }
    }
    None
}

fn detach(n: &mut Node, id: u64) -> Option<Node> {
    if let Some(i) = n.children.iter().position(|c| c.id == id) {
        return Some(n.children.remove(i));
    }
    for c in &mut n.children {
        if let Some(found) = detach(c, id) {
            return Some(found);
        }
    }
    None
}

fn contains_id(n: &Node, id: u64) -> bool {
    n.id == id || n.children.iter().any(|c| contains_id(c, id))
}

fn max_id(n: &Node) -> u64 {
    n.children.iter().map(max_id).max().unwrap_or(0).max(n.id)
}

// ---------------------------------------------------------------- layout ----

fn node_metrics(text: &str, depth: usize, ctx: &egui::Context) -> Vec2 {
    let (font, pad, min_w, max_w) = match depth {
        0 => (19.0, 24.0, 118.0, 310.0),
        1 => (15.0, 20.0, 100.0, 235.0),
        _ => (14.0, 17.0, 92.0, 215.0),
    };
    let galley = ctx.fonts_mut(|f| {
        f.layout_no_wrap(text.to_owned(), FontId::proportional(font), Color32::WHITE)
    });
    let w = (galley.size().x + pad * 2.0).clamp(min_w, max_w);
    let h = match depth {
        0 => 58.0,
        1 => 44.0,
        _ => 38.0,
    };
    Vec2::new(w, h)
}

fn subtree_height(node: &Node, depth: usize, ctx: &egui::Context) -> f32 {
    let own = node_metrics(&node.text, depth, ctx).y;
    if node.children.is_empty() {
        return own;
    }
    let children: f32 = node
        .children
        .iter()
        .map(|c| subtree_height(c, depth + 1, ctx) + V_GAP)
        .sum::<f32>()
        - V_GAP;
    own.max(children)
}

fn place_branch(
    node: &Node,
    center: Pos2,
    depth: usize,
    color: usize,
    parent: u64,
    sign: f32,
    ctx: &egui::Context,
    out: &mut Vec<Laid>,
) {
    let size = node_metrics(&node.text, depth, ctx);
    let rect = Rect::from_center_size(center, size);
    out.push(Laid {
        id: node.id,
        text: node.text.clone(),
        rect,
        depth,
        color,
        parent: Some(parent),
    });

    if node.children.is_empty() {
        return;
    }

    let total = node
        .children
        .iter()
        .map(|c| subtree_height(c, depth + 1, ctx) + V_GAP)
        .sum::<f32>()
        - V_GAP;

    let mut y = center.y - total * 0.5;
    let gap = if depth == 0 { 92.0 } else { 58.0 };

    for child in &node.children {
        let extent = subtree_height(child, depth + 1, ctx) + V_GAP;
        let cy = y + extent * 0.5;
        let child_size = node_metrics(&child.text, depth + 1, ctx);
        let cx = center.x + sign * (size.x * 0.5 + gap + child_size.x * 0.5);
        place_branch(
            child,
            Pos2::new(cx, cy),
            depth + 1,
            color,
            node.id,
            sign,
            ctx,
            out,
        );
        y += extent;
    }
}

fn build_layout(root: &Node, ctx: &egui::Context) -> Vec<Laid> {
    let mut out = Vec::new();
    let root_size = node_metrics(&root.text, 0, ctx);
    out.push(Laid {
        id: root.id,
        text: root.text.clone(),
        rect: Rect::from_center_size(Pos2::ZERO, root_size),
        depth: 0,
        color: 0,
        parent: None,
    });

    let right: Vec<(usize, &Node)> = root
        .children
        .iter()
        .enumerate()
        .filter(|(i, _)| i % 2 == 0)
        .collect();
    let left: Vec<(usize, &Node)> = root
        .children
        .iter()
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .collect();

    for (nodes, sign) in [(right, 1.0_f32), (left, -1.0_f32)] {
        if nodes.is_empty() {
            continue;
        }
        let total = nodes
            .iter()
            .map(|(_, n)| subtree_height(n, 1, ctx) + BRANCH_GAP)
            .sum::<f32>()
            - BRANCH_GAP;
        let mut y = -total * 0.5;
        for (index, child) in nodes {
            let extent = subtree_height(child, 1, ctx) + BRANCH_GAP;
            let cy = y + extent * 0.5;
            let child_size = node_metrics(&child.text, 1, ctx);
            let cx = sign * (root_size.x * 0.5 + 92.0 + child_size.x * 0.5);
            place_branch(
                child,
                Pos2::new(cx, cy),
                1,
                index,
                root.id,
                sign,
                ctx,
                &mut out,
            );
            y += extent;
        }
    }

    out
}

// ---------------------------------------------------------------- helpers ---

fn cr(radius: f32) -> CornerRadius {
    CornerRadius::same(radius.clamp(0.0, 255.0) as u8)
}

fn tint(color: Color32, amount: f32) -> Color32 {
    let f =
        |v: u8| -> u8 { (v as f32 + (255.0 - v as f32) * amount.clamp(0.0, 1.0)).round() as u8 };
    Color32::from_rgb(f(color.r()), f(color.g()), f(color.b()))
}

fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

fn elide(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_owned();
    }
    let take = max_chars.saturating_sub(1);
    let mut out: String = text.chars().take(take).collect();
    out.push('…');
    out
}

fn branch_color(index: usize) -> Color32 {
    BRANCH[index % BRANCH.len()]
}

// ------------------------------------------------------------------ pdf -----

fn pdf_rgb(color: Color32) -> PdfColor {
    PdfColor::Rgb(PdfRgb::new(
        color.r() as f32 / 255.0,
        color.g() as f32 / 255.0,
        color.b() as f32 / 255.0,
        None,
    ))
}

fn pdf_point(x: f32, y: f32) -> PdfPoint {
    PdfPoint {
        x: PdfPt(x),
        y: PdfPt(y),
    }
}

fn pdf_line_point(x: f32, y: f32, bezier: bool) -> LinePoint {
    LinePoint {
        p: pdf_point(x, y),
        bezier,
    }
}

/// Скруглённый прямоугольник как полигон с безье-углами (координаты в pt, Y вверх).
fn rounded_rect_polygon(
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    radius: f32,
    mode: PaintMode,
) -> Polygon {
    let r = radius
        .min((x1 - x0).abs() * 0.5)
        .min((y1 - y0).abs() * 0.5)
        .max(0.0);
    let mut points = Vec::new();
    if r < 0.75 {
        for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
            points.push(pdf_line_point(x, y, false));
        }
    } else {
        let k = r * 0.5523;
        points.push(pdf_line_point(x0 + r, y0, false));
        points.push(pdf_line_point(x1 - r + k, y0, true));
        points.push(pdf_line_point(x1, y0 + r - k, true));
        points.push(pdf_line_point(x1, y0 + r, false));
        points.push(pdf_line_point(x1, y1 - r + k, true));
        points.push(pdf_line_point(x1 - r + k, y1, true));
        points.push(pdf_line_point(x1 - r, y1, false));
        points.push(pdf_line_point(x0 + r - k, y1, true));
        points.push(pdf_line_point(x0, y1 - r + k, true));
        points.push(pdf_line_point(x0, y1 - r, false));
        points.push(pdf_line_point(x0, y0 + r - k, true));
        points.push(pdf_line_point(x0 + r - k, y0, true));
        points.push(pdf_line_point(x0 + r, y0, false));
    }
    Polygon {
        rings: vec![PolygonRing { points }],
        mode,
        winding_order: WindingOrder::NonZero,
    }
}

fn measure_text_width(ctx: &egui::Context, text: &str, size: f32) -> f32 {
    ctx.fonts_mut(|f| {
        f.layout_no_wrap(text.to_owned(), FontId::proportional(size), Color32::WHITE)
            .size()
            .x
    })
}

/// Обрезает текст по фактической ширине (в pt), добавляя «…».
fn elide_to_width(ctx: &egui::Context, text: &str, size: f32, max_w: f32) -> String {
    if measure_text_width(ctx, text, size) <= max_w {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut best = String::from("…");
    for take in 1..chars.len() {
        let mut candidate: String = chars[..take].iter().collect();
        candidate.push('…');
        if measure_text_width(ctx, &candidate, size) <= max_w {
            best = candidate;
        } else {
            break;
        }
    }
    best
}

fn save_png(path: &PathBuf, image: &egui::ColorImage) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut bytes = Vec::with_capacity(image.pixels.len() * 4);
    for pixel in &image.pixels {
        bytes.extend_from_slice(&pixel.to_array());
    }
    match image::save_buffer(
        path,
        &bytes,
        image.size[0] as u32,
        image.size[1] as u32,
        image::ColorType::Rgba8,
    ) {
        Ok(()) => eprintln!("Screenshot saved to {path:?}"),
        Err(err) => eprintln!("Could not save screenshot to {path:?}: {err}"),
    }
}

fn parse_screenshot_arg() -> Option<PathBuf> {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--screenshot" {
            return args.next().map(PathBuf::from);
        }
        if let Some(value) = arg.strip_prefix("--screenshot=") {
            return Some(PathBuf::from(value));
        }
    }
    None
}

fn parse_pdf_arg() -> Option<PathBuf> {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--pdf" {
            return args.next().map(PathBuf::from);
        }
        if let Some(value) = arg.strip_prefix("--pdf=") {
            return Some(PathBuf::from(value));
        }
    }
    None
}

// ------------------------------------------------------------------- app ----

impl MindMapApp {
    fn new(cc: &eframe::CreationContext<'_>, shot: Option<PathBuf>, pdf: Option<PathBuf>) -> Self {
        cc.egui_ctx.set_theme(egui::ThemePreference::Light);
        let mut style = (*cc.egui_ctx.style_of(egui::Theme::Light)).clone();
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.visuals.override_text_color = Some(TEXT);
        style.visuals.selection.bg_fill = with_alpha(ACCENT, 55);
        style.visuals.selection.stroke = Stroke::new(1.0, ACCENT);
        cc.egui_ctx.set_style_of(egui::Theme::Light, style);

        let save_path = env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("mindmap.json");

        let loaded = fs::read_to_string(&save_path)
            .ok()
            .and_then(|text| serde_json::from_str::<Node>(&text).ok());

        let (doc, save_ok) = match loaded {
            Some(doc) if !doc.text.trim().is_empty() => (doc, true),
            _ => (sample_map(), false),
        };

        let next_id = max_id(&doc) + 1;
        let selected = doc.id;

        let mut app = Self {
            doc,
            next_id,
            selected,
            editing: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            camera: Camera::default(),
            drag: None,
            panning: false,
            hovered: None,
            needs_fit: true,
            save_ok,
            save_path,
            toast: None,
            shot: shot.map(|path| ShotState {
                path,
                frame: 0,
                requested: false,
            }),
            auto_pdf: pdf.map(|path| (path, 0)),
            pdf_dir: None,
        };

        if !app.save_ok {
            app.save();
        }
        app
    }

    fn save(&mut self) {
        let json = match serde_json::to_string_pretty(&self.doc) {
            Ok(json) => json,
            Err(_) => {
                self.save_ok = false;
                return;
            }
        };
        self.save_ok = fs::write(&self.save_path, json).is_ok();
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(self.doc.clone());
        if self.undo_stack.len() > 80 {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn undo(&mut self) {
        if let Some(previous) = self.undo_stack.pop() {
            self.redo_stack.push(self.doc.clone());
            self.doc = previous;
            self.after_history_change();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.doc.clone());
            self.doc = next;
            self.after_history_change();
        }
    }

    fn after_history_change(&mut self) {
        if find(&self.doc, self.selected).is_none() {
            self.selected = self.doc.id;
        }
        self.editing = None;
        self.drag = None;
        self.save();
    }

    fn new_map(&mut self) {
        self.push_undo();
        self.doc = empty_map();
        self.next_id = 2;
        self.selected = self.doc.id;
        self.editing = None;
        self.drag = None;
        self.needs_fit = true;
        self.save();
    }

    fn add_child(&mut self, parent_id: u64) {
        if find(&self.doc, parent_id).is_none() {
            return;
        }
        self.push_undo();
        let id = self.next_id;
        self.next_id += 1;
        if let Some(parent) = find_mut(&mut self.doc, parent_id) {
            parent.children.push(node(id, "Новая ветка", vec![]));
        }
        self.selected = id;
        self.save();
        self.begin_edit(id);
    }

    fn add_sibling(&mut self) {
        if self.selected == self.doc.id {
            self.add_child(self.doc.id);
            return;
        }
        let parent = parent_of(&self.doc, self.selected).unwrap_or(self.doc.id);
        self.add_child(parent);
    }

    fn delete_selected(&mut self) {
        if self.selected == self.doc.id {
            return;
        }
        let parent = match parent_of(&self.doc, self.selected) {
            Some(parent) => parent,
            None => return,
        };
        self.push_undo();
        detach(&mut self.doc, self.selected);
        self.selected = parent;
        self.editing = None;
        self.drag = None;
        self.save();
    }

    fn begin_edit(&mut self, id: u64) {
        if self
            .editing
            .as_ref()
            .map(|edit| edit.id != id)
            .unwrap_or(false)
        {
            self.commit_edit();
        }
        if let Some(found) = find(&self.doc, id) {
            self.selected = id;
            self.editing = Some(EditState {
                id,
                text: found.text.clone(),
                select_all: true,
            });
        }
    }

    fn commit_edit(&mut self) {
        let Some(edit) = self.editing.take() else {
            return;
        };
        let cleaned = edit.text.trim().to_owned();
        let final_text = if cleaned.is_empty() {
            "Без названия".to_owned()
        } else {
            cleaned
        };
        let changed = find(&self.doc, edit.id)
            .map(|n| n.text != final_text)
            .unwrap_or(false);
        if changed {
            self.push_undo();
            if let Some(found) = find_mut(&mut self.doc, edit.id) {
                found.text = final_text;
            }
            self.save();
        }
    }

    fn perform_reparent(&mut self, id: u64, target: Option<u64>) {
        let Some(target) = target else {
            return;
        };
        if target == id {
            return;
        }
        if find(&self.doc, id)
            .map(|n| contains_id(n, target))
            .unwrap_or(true)
        {
            return;
        }
        if parent_of(&self.doc, id) == Some(target) {
            return;
        }
        self.push_undo();
        if let Some(subtree) = detach(&mut self.doc, id) {
            if let Some(parent) = find_mut(&mut self.doc, target) {
                parent.children.push(subtree);
            }
        }
        self.save();
    }

    fn fit_to(&mut self, layout: &[Laid], view: Vec2) {
        if layout.is_empty() {
            return;
        }
        let mut bounds = layout[0].rect;
        for laid in layout.iter().skip(1) {
            bounds = bounds.union(laid.rect);
        }
        let zoom_x = (view.x - 150.0) / bounds.width().max(1.0);
        let zoom_y = (view.y - 140.0) / bounds.height().max(1.0);
        self.camera.zoom = zoom_x.min(zoom_y).clamp(0.35, 1.25);
        self.camera.pan = -bounds.center().to_vec2();
    }

    fn zoom_at(&mut self, pointer: Pos2, canvas: Rect, factor: f32) {
        let old = self.camera.zoom;
        let new = (old * factor).clamp(0.3, 2.6);
        if (new - old).abs() < f32::EPSILON {
            return;
        }
        let center = canvas.center();
        let before = (pointer - center) / old - self.camera.pan;
        self.camera.zoom = new;
        self.camera.pan = (pointer - center) / new - before;
    }

    fn apply_action(&mut self, action: Action, ctx: &egui::Context) {
        if self.editing.is_some() {
            self.commit_edit();
        }
        match action {
            Action::AddChild => {
                let parent = self.selected;
                self.add_child(parent);
            }
            Action::NewMap => self.new_map(),
            Action::Delete => self.delete_selected(),
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::Fit => self.needs_fit = true,
            Action::ZoomIn => {
                self.camera.zoom = (self.camera.zoom * 1.2).clamp(0.3, 2.6);
            }
            Action::ZoomOut => {
                self.camera.zoom = (self.camera.zoom / 1.2).clamp(0.3, 2.6);
            }
            Action::ExportPdf => {
                let Some(pdf_path) = self.choose_pdf_path() else {
                    self.toast = Some(("Экспорт PDF отменён".to_owned(), Instant::now()));
                    return;
                };
                if let Some(dir) = pdf_path.parent() {
                    self.pdf_dir = Some(dir.to_path_buf());
                }
                match self.export_pdf(ctx, &pdf_path) {
                    Ok(path) => {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "mindmap.pdf".to_owned());
                        self.toast = Some((format!("PDF сохранён: {name}"), Instant::now()));
                    }
                    Err(err) => {
                        self.toast =
                            Some((format!("Не удалось сохранить PDF: {err}"), Instant::now()));
                    }
                }
            }
        }
    }

    /// Открывает системный диалог выбора пути для сохранения PDF.
    /// Возвращает `None`, если пользователь отменил выбор.
    fn choose_pdf_path(&self) -> Option<PathBuf> {
        let start_dir = self
            .pdf_dir
            .clone()
            .or_else(|| self.save_path.parent().map(|p| p.to_path_buf()));

        let mut dialog = rfd::FileDialog::new()
            .set_title("Сохранить карту в PDF")
            .add_filter("PDF", &["pdf"])
            .set_file_name("mindmap.pdf");
        if let Some(dir) = start_dir {
            dialog = dialog.set_directory(dir);
        }

        let chosen = dialog.save_file()?;

        // Принудительно выставляем расширение .pdf.
        let mut path = chosen;
        let has_pdf_ext = path
            .extension()
            .map(|e| e.eq_ignore_ascii_case("pdf"))
            .unwrap_or(false);
        if !has_pdf_ext {
            path.set_extension("pdf");
        }
        Some(path)
    }

    fn export_pdf(&mut self, ctx: &egui::Context, path: &Path) -> Result<PathBuf, String> {
        let layout = build_layout(&self.doc, ctx);
        if layout.is_empty() {
            return Err("карта пуста".to_owned());
        }
        let path = path.to_path_buf();

        // Границы карты в мировых (экранных) точках.
        let mut bounds = layout[0].rect;
        for laid in layout.iter().skip(1) {
            bounds = bounds.union(laid.rect);
        }

        const MARGIN: f32 = 36.0;
        const PAD: f32 = 14.0;
        const MAX_W: f32 = 1584.0;
        const MAX_H: f32 = 1224.0;

        let content_w = bounds.width() + PAD * 2.0;
        let content_h = bounds.height() + PAD * 2.0;
        let mut scale = 1.0_f32;
        if content_w + MARGIN * 2.0 > MAX_W {
            scale = scale.min((MAX_W - MARGIN * 2.0) / content_w);
        }
        if content_h + MARGIN * 2.0 > MAX_H {
            scale = scale.min((MAX_H - MARGIN * 2.0) / content_h);
        }
        scale = scale.clamp(0.12, 1.0);

        let page_w = (content_w * scale + MARGIN * 2.0).max(180.0);
        let page_h = (content_h * scale + MARGIN * 2.0).max(120.0);

        // world -> pdf (ось Y перевёрнута: в PDF начало снизу слева).
        let min = bounds.min - Vec2::splat(PAD);
        let map_x = |x: f32| (x - min.x) * scale + MARGIN;
        let map_y = |y: f32| page_h - ((y - min.y) * scale + MARGIN);

        let mut font_warnings = Vec::new();
        let font = ParsedFont::from_bytes(UBUNTU_LIGHT, 0, &mut font_warnings)
            .ok_or_else(|| "не удалось разобрать шрифт".to_owned())?;

        let mut doc = PdfDocument::new("Карта мыслей");
        let font_id = doc.add_font(&font);
        let mut ops: Vec<PdfOp> = Vec::new();

        // Белый фон страницы.
        ops.push(PdfOp::SetFillColor {
            col: pdf_rgb(WHITE),
        });
        ops.push(PdfOp::DrawRectangle {
            rectangle: PdfRect {
                x: PdfPt(0.0),
                y: PdfPt(0.0),
                width: PdfPt(page_w),
                height: PdfPt(page_h),
                mode: Some(PaintMode::Fill),
                winding_order: Some(WindingOrder::NonZero),
            },
        });

        // Связи между узлами.
        let rects: HashMap<u64, Rect> = layout.iter().map(|l| (l.id, l.rect)).collect();
        for laid in &layout {
            let Some(parent_id) = laid.parent else {
                continue;
            };
            let Some(parent) = rects.get(&parent_id).copied() else {
                continue;
            };
            let child = laid.rect;
            let (p0, p3) = if child.center().x >= parent.center().x {
                (
                    Pos2::new(parent.right() - 1.0, parent.center().y),
                    Pos2::new(child.left() + 1.0, child.center().y),
                )
            } else {
                (
                    Pos2::new(parent.left() + 1.0, parent.center().y),
                    Pos2::new(child.right() - 1.0, child.center().y),
                )
            };
            let dx = (p3.x - p0.x) * 0.55;
            ops.push(PdfOp::SetOutlineColor {
                col: pdf_rgb(branch_color(laid.color)),
            });
            ops.push(PdfOp::SetOutlineThickness {
                pt: PdfPt((1.8 * scale).max(0.8)),
            });
            ops.push(PdfOp::DrawLine {
                line: PdfLine {
                    points: vec![
                        pdf_line_point(map_x(p0.x), map_y(p0.y), false),
                        pdf_line_point(map_x(p0.x + dx), map_y(p0.y), true),
                        pdf_line_point(map_x(p3.x - dx), map_y(p3.y), true),
                        pdf_line_point(map_x(p3.x), map_y(p3.y), false),
                    ],
                    is_closed: false,
                },
            });
        }

        // Узлы.
        for laid in &layout {
            let x0 = map_x(laid.rect.left());
            let x1 = map_x(laid.rect.right());
            let y_bottom = map_y(laid.rect.bottom());
            let y_top = map_y(laid.rect.top());
            let color = branch_color(laid.color);
            let radius = self.node_radius(laid.depth) * scale;

            let (fill, border, text_color) = match laid.depth {
                0 => (NAVY, None, WHITE),
                1 => (color, None, WHITE),
                _ => (tint(color, 0.88), Some(color), TEXT),
            };

            ops.push(PdfOp::SetFillColor { col: pdf_rgb(fill) });
            let mode = if let Some(border) = border {
                ops.push(PdfOp::SetOutlineColor {
                    col: pdf_rgb(border),
                });
                ops.push(PdfOp::SetOutlineThickness {
                    pt: PdfPt((1.0 * scale).max(0.5)),
                });
                PaintMode::FillStroke
            } else {
                PaintMode::Fill
            };
            ops.push(PdfOp::DrawPolygon {
                polygon: rounded_rect_polygon(x0, y_bottom, x1, y_top, radius, mode),
            });

            // Текст по центру узла.
            // Решение об обрезке принимаем в НЕмасштабированных единицах — тем же
            // шрифтом и той же шириной, что использовал layout при расчёте узла.
            // Иначе при scale < 1 (крупные карты) округление размеров шрифта в egui
            // делает измерение нелинейным и текст обрезается, хотя он помещается.
            let font_size = laid_font(laid.depth) * scale;
            let pad = match laid.depth {
                0 => 24.0,
                1 => 20.0,
                _ => 17.0,
            };
            let max_text_w = (laid.rect.width() - pad * 2.0).max(4.0);
            let text = elide_to_width(ctx, &laid.text, laid_font(laid.depth), max_text_w + 0.5);
            let text_w = measure_text_width(ctx, &text, font_size);
            let tx = (x0 + x1) * 0.5 - text_w * 0.5;
            let ty = (y_bottom + y_top) * 0.5 - font_size * 0.35;

            ops.push(PdfOp::StartTextSection);
            ops.push(PdfOp::SetFillColor {
                col: pdf_rgb(text_color),
            });
            ops.push(PdfOp::SetFont {
                font: PdfFontHandle::External(font_id.clone()),
                size: PdfPt(font_size),
            });
            ops.push(PdfOp::SetTextCursor {
                pos: pdf_point(tx, ty),
            });
            ops.push(PdfOp::ShowText {
                items: vec![TextItem::Text(text)],
            });
            ops.push(PdfOp::EndTextSection);
        }

        doc.with_pages(vec![PdfPage::new(
            Mm::from(PdfPt(page_w)),
            Mm::from(PdfPt(page_h)),
            ops,
        )]);

        let mut save_warnings = Vec::new();
        let bytes = doc.save(&PdfSaveOptions::default(), &mut save_warnings);
        fs::write(&path, bytes).map_err(|err| format!("{err}"))?;
        Ok(path)
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if self.editing.is_some() {
            let (escape, enter) = ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::Escape),
                    i.consume_key(Modifiers::NONE, Key::Enter),
                )
            });
            if escape {
                self.editing = None;
            } else if enter {
                self.commit_edit();
            }
            return;
        }

        let mut add_child = false;
        let mut add_sibling = false;
        let mut delete = false;
        let mut edit = false;
        let mut undo = false;
        let mut redo = false;
        let mut new_map = false;
        let mut fit = false;
        let mut export = false;
        let shift_cmd = Modifiers {
            command: true,
            shift: true,
            ..Default::default()
        };

        ctx.input_mut(|i| {
            undo = i.consume_key(Modifiers::COMMAND, Key::Z);
            redo = i.consume_key(shift_cmd, Key::Z) || i.consume_key(Modifiers::COMMAND, Key::Y);
            new_map = i.consume_key(Modifiers::COMMAND, Key::N);
            fit = i.consume_key(Modifiers::NONE, Key::F);
            export = i.consume_key(Modifiers::COMMAND, Key::P);
            add_child = i.consume_key(Modifiers::NONE, Key::Tab);
            add_sibling = i.consume_key(Modifiers::NONE, Key::Enter);
            delete = i.consume_key(Modifiers::NONE, Key::Delete)
                || i.consume_key(Modifiers::NONE, Key::Backspace);
            edit = i.consume_key(Modifiers::NONE, Key::F2);
        });

        if undo {
            self.undo();
        }
        if redo {
            self.redo();
        }
        if new_map {
            self.new_map();
        }
        if fit {
            self.needs_fit = true;
        }
        if export {
            self.apply_action(Action::ExportPdf, ctx);
        }
        if add_child {
            let parent = self.selected;
            self.add_child(parent);
            return;
        }
        if add_sibling {
            self.add_sibling();
            return;
        }
        if edit {
            self.begin_edit(self.selected);
            return;
        }
        if delete {
            self.delete_selected();
        }
    }

    // ------------------------------------------------------------- canvas ---

    fn canvas_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let (canvas_rect, bg) =
            ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        if canvas_rect.width() < 10.0 || canvas_rect.height() < 10.0 {
            return;
        }

        let layout = build_layout(&self.doc, ctx);
        if self.needs_fit {
            let size = canvas_rect.size();
            self.fit_to(&layout, size);
            self.needs_fit = false;
        }

        let pointer_pos = ctx.input(|i| i.pointer.hover_pos());
        let over_node = pointer_pos
            .map(|p| {
                layout
                    .iter()
                    .any(|l| self.map_rect(l.rect, canvas_rect).contains(p))
            })
            .unwrap_or(false);

        if bg.drag_started() {
            self.panning = !over_node && self.drag.is_none();
        }
        if bg.dragged() && self.panning {
            self.camera.pan += bg.drag_delta() / self.camera.zoom;
        }
        if bg.drag_stopped() {
            self.panning = false;
        }

        if let Some(pointer) = pointer_pos {
            if canvas_rect.contains(pointer)
                && !self.panning
                && self.drag.is_none()
                && self.editing.is_none()
            {
                let (scroll, zoom_delta) = ctx.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
                if (zoom_delta - 1.0).abs() > 0.001 {
                    self.zoom_at(pointer, canvas_rect, zoom_delta);
                } else if scroll.abs() > 0.1 {
                    let factor = (scroll * 0.0016).exp();
                    self.zoom_at(pointer, canvas_rect, factor);
                }
            }
        }

        let camera = Camera {
            pan: self.camera.pan,
            zoom: self.camera.zoom,
        };
        let to_screen = |p: Pos2| canvas_rect.center() + (p.to_vec2() + camera.pan) * camera.zoom;
        let screen_rect = |r: Rect| Rect::from_min_max(to_screen(r.min), to_screen(r.max));

        let painter = ui.painter_at(canvas_rect);
        painter.rect_filled(canvas_rect, 0, CANVAS);

        // Soft dot grid.
        let spacing = 28.0 * camera.zoom;
        if spacing > 11.0 {
            let origin = canvas_rect.center() + camera.pan * camera.zoom;
            let dot = Color32::from_rgba_unmultiplied(139, 148, 163, 58);
            let base_x = origin.x - ((origin.x - canvas_rect.left()) / spacing).floor() * spacing;
            let base_y = origin.y - ((origin.y - canvas_rect.top()) / spacing).floor() * spacing;
            let nx = ((canvas_rect.width() / spacing).ceil() as i32 + 2).min(220);
            let ny = ((canvas_rect.height() / spacing).ceil() as i32 + 2).min(220);
            for iy in 0..ny {
                for ix in 0..nx {
                    let p = Pos2::new(base_x + ix as f32 * spacing, base_y + iy as f32 * spacing);
                    if canvas_rect.contains(p) {
                        painter.circle_filled(p, 1.05, dot);
                    }
                }
            }
        }

        let mut rects: HashMap<u64, Rect> = HashMap::new();
        for laid in &layout {
            rects.insert(laid.id, screen_rect(laid.rect));
        }

        // Connections.
        for laid in &layout {
            let Some(parent_id) = laid.parent else {
                continue;
            };
            let Some(pr) = rects.get(&parent_id).copied() else {
                continue;
            };
            let Some(cr_) = rects.get(&laid.id).copied() else {
                continue;
            };
            let (p0, p3) = if cr_.center().x >= pr.center().x {
                (
                    Pos2::new(pr.right() - 1.0, pr.center().y),
                    Pos2::new(cr_.left() + 1.0, cr_.center().y),
                )
            } else {
                (
                    Pos2::new(pr.left() + 1.0, pr.center().y),
                    Pos2::new(cr_.right() - 1.0, cr_.center().y),
                )
            };
            let dx = (p3.x - p0.x) * 0.55;
            let color = branch_color(laid.color);
            let stroke = Stroke::new((2.0 * camera.zoom).clamp(1.1, 2.8), with_alpha(color, 175));
            let curve = egui::epaint::CubicBezierShape::from_points_stroke(
                [
                    p0,
                    Pos2::new(p0.x + dx, p0.y),
                    Pos2::new(p3.x - dx, p3.y),
                    p3,
                ],
                false,
                Color32::TRANSPARENT,
                stroke,
            );
            painter.add(curve);
        }

        // Nodes.
        let selected = self.selected;
        let editing_id = self.editing.as_ref().map(|e| e.id);
        for laid in &layout {
            let Some(rect) = rects.get(&laid.id).copied() else {
                continue;
            };
            self.draw_node(
                &painter,
                laid,
                rect,
                camera.zoom,
                selected == laid.id,
                editing_id == Some(laid.id),
            );
        }

        // Node interaction.
        let mut drag_stopped = false;
        let mut hovered = None;
        for laid in &layout {
            let Some(rect) = rects.get(&laid.id).copied() else {
                continue;
            };
            let response = ui.interact(rect, Id::new(("node", laid.id)), Sense::click_and_drag());
            if response.hovered() && self.drag.is_none() {
                hovered = Some(laid.id);
            }
            if response.clicked() {
                self.selected = laid.id;
            }
            if response.double_clicked() {
                self.begin_edit(laid.id);
            }
            if response.drag_started() {
                self.selected = laid.id;
                self.drag = Some(laid.id);
                self.panning = false;
            }
            if response.drag_stopped() {
                drag_stopped = true;
            }
        }
        self.hovered = hovered;

        let drop_target = self.drop_target(&layout, &rects, pointer_pos, canvas_rect);

        if drag_stopped {
            if let Some(id) = self.drag.take() {
                self.perform_reparent(id, drop_target);
            }
        }

        // Drop highlight.
        if let (Some(target), Some(_)) = (drop_target, self.drag) {
            if let Some(rect) = rects.get(&target).copied() {
                let laid = layout.iter().find(|l| l.id == target);
                let radius = laid.map(|l| self.node_radius(l.depth)).unwrap_or(10.0);
                painter.rect_stroke(
                    rect.expand(4.0),
                    cr(radius + 3.0),
                    Stroke::new(2.6, ACCENT),
                    StrokeKind::Outside,
                );
            }
        }

        // Dragged topic ghost.
        if let (Some(id), Some(pointer)) = (self.drag, pointer_pos) {
            if let Some(laid) = layout.iter().find(|l| l.id == id) {
                if let Some(rect) = rects.get(&id).copied() {
                    let size = rect.size() * 0.96;
                    let ghost = Rect::from_center_size(pointer + Vec2::new(0.0, -6.0), size);
                    let fill = if laid.depth == 0 {
                        NAVY
                    } else if laid.depth == 1 {
                        branch_color(laid.color)
                    } else {
                        tint(branch_color(laid.color), 0.86)
                    };
                    painter.rect_filled(
                        ghost.translate(Vec2::new(0.0, 5.0)),
                        cr(self.node_radius(laid.depth)),
                        Color32::from_black_alpha(35),
                    );
                    painter.rect(
                        ghost,
                        cr(self.node_radius(laid.depth)),
                        with_alpha(fill, 235),
                        Stroke::new(1.2, with_alpha(branch_color(laid.color), 160)),
                        StrokeKind::Inside,
                    );
                    let color = if laid.depth <= 1 { WHITE } else { TEXT };
                    let font = laid_font(laid.depth) * camera.zoom;
                    painter.text(
                        ghost.center(),
                        Align2::CENTER_CENTER,
                        elide(&laid.text, 26),
                        FontId::proportional(font),
                        color,
                    );
                }
            }
        }

        if let Some(pointer) = pointer_pos {
            if canvas_rect.contains(pointer) {
                if self.drag.is_some() || self.panning {
                    ctx.set_cursor_icon(CursorIcon::Grabbing);
                } else if self.hovered.is_some() {
                    ctx.set_cursor_icon(CursorIcon::PointingHand);
                } else {
                    ctx.set_cursor_icon(CursorIcon::Grab);
                }
            }
        }

        if self.editing.is_some() {
            self.show_editor(ctx, &layout, &rects, canvas_rect);
        }
    }

    fn node_radius(&self, depth: usize) -> f32 {
        match depth {
            0 => 16.0,
            1 => 12.0,
            _ => 10.0,
        }
    }

    fn map_rect(&self, r: Rect, canvas: Rect) -> Rect {
        let to_screen =
            |p: Pos2| canvas.center() + (p.to_vec2() + self.camera.pan) * self.camera.zoom;
        Rect::from_min_max(to_screen(r.min), to_screen(r.max))
    }

    fn drop_target(
        &self,
        layout: &[Laid],
        rects: &HashMap<u64, Rect>,
        pointer: Option<Pos2>,
        canvas: Rect,
    ) -> Option<u64> {
        let dragged = self.drag?;
        let pointer = pointer?;
        if !canvas.contains(pointer) {
            return None;
        }
        let subtree = find(&self.doc, dragged)?;
        layout
            .iter()
            .filter(|l| l.id != dragged && !contains_id(subtree, l.id))
            .find(|l| {
                rects
                    .get(&l.id)
                    .map(|r| r.contains(pointer))
                    .unwrap_or(false)
            })
            .map(|l| l.id)
    }

    fn draw_node(
        &self,
        painter: &egui::Painter,
        laid: &Laid,
        rect: Rect,
        zoom: f32,
        selected: bool,
        editing: bool,
    ) {
        let color = branch_color(laid.color);
        let radius = self.node_radius(laid.depth);
        let (fill, text_color, border) = match laid.depth {
            0 => (NAVY, WHITE, None),
            1 => (color, WHITE, None),
            _ => (tint(color, 0.88), TEXT, Some(with_alpha(color, 120))),
        };

        painter.rect_filled(
            rect.translate(Vec2::new(0.0, 3.0 * zoom)),
            cr(radius),
            Color32::from_black_alpha(18),
        );
        painter.rect(
            rect,
            cr(radius),
            fill,
            border
                .map(|c| Stroke::new((1.2 * zoom).clamp(1.0, 2.0), c))
                .unwrap_or(Stroke::NONE),
            StrokeKind::Inside,
        );

        if selected {
            painter.rect_stroke(
                rect.expand(3.5),
                cr(radius + 3.0),
                Stroke::new(2.0, ACCENT),
                StrokeKind::Outside,
            );
        }

        if editing {
            return;
        }

        let size = laid_font(laid.depth) * zoom;
        let pad = match laid.depth {
            0 => 24.0,
            1 => 20.0,
            _ => 17.0,
        };
        let max_chars = (((rect.width() - 2.0 * pad) / (size * 0.56)).floor() as usize).max(3);
        let text = elide(&laid.text, max_chars);
        let clipped = painter.with_clip_rect(rect.shrink(2.0));
        let galley = clipped.layout_no_wrap(text, FontId::proportional(size), text_color);
        clipped.galley(rect.center() - galley.size() * 0.5, galley, text_color);
    }

    fn show_editor(
        &mut self,
        ctx: &egui::Context,
        layout: &[Laid],
        rects: &HashMap<u64, Rect>,
        canvas: Rect,
    ) {
        let Some(mut edit) = self.editing.take() else {
            return;
        };
        let Some(laid) = layout.iter().find(|l| l.id == edit.id) else {
            return;
        };
        let Some(rect) = rects.get(&edit.id).copied() else {
            return;
        };
        if !rect.intersects(canvas) {
            self.editing = Some(edit);
            return;
        }

        let zoom = self.camera.zoom;
        let font_size = laid_font(laid.depth) * zoom;
        let text_color = if laid.depth <= 1 { WHITE } else { TEXT };
        let pad = match laid.depth {
            0 => 24.0,
            1 => 20.0,
            _ => 17.0,
        } * zoom;
        let width = (rect.width() - pad * 2.0).max(30.0);
        let pos = Pos2::new(rect.left() + pad, rect.center().y - font_size * 0.72);
        let edit_id = Id::new(("topic_editor", edit.id));
        let just_opened = edit.select_all;
        let mut response = None;
        let mut state = None;

        egui::Area::new(Id::new(("topic_editor_area", edit.id)))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                ui.set_width(width);
                let output = egui::TextEdit::singleline(&mut edit.text)
                    .id(edit_id)
                    .font(FontId::proportional(font_size))
                    .text_color(text_color)
                    .frame(Frame::NONE)
                    .margin(Margin::ZERO)
                    .desired_width(width)
                    .vertical_align(Align::Center)
                    .show(ui);
                response = Some(output.response.clone());
                state = Some(output.state);
            });

        if just_opened {
            ctx.memory_mut(|m| m.request_focus(edit_id));
            if let Some(mut state) = state {
                let count = edit.text.chars().count();
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(count),
                    )));
                state.store(ctx, edit_id);
            }
            edit.select_all = false;
        }

        let lost_focus = response.map(|r| r.lost_focus()).unwrap_or(false);
        if lost_focus {
            self.editing = Some(edit);
            self.commit_edit();
        } else {
            self.editing = Some(edit);
        }
    }

    // ---------------------------------------------------------- screenshot ---

    fn handle_screenshot_event(&mut self, ctx: &egui::Context) {
        let Some(shot) = &self.shot else {
            return;
        };
        let image = ctx.input(|i| {
            i.events.iter().find_map(|event| {
                if let egui::Event::Screenshot { image, .. } = event {
                    Some(image.clone())
                } else {
                    None
                }
            })
        });
        if let Some(image) = image {
            save_png(&shot.path, &image);
            std::process::exit(0);
        }
    }

    fn screenshot_tick(&mut self, ctx: &egui::Context) {
        let Some(shot) = &mut self.shot else {
            return;
        };
        shot.frame += 1;
        if shot.frame >= 120 && (!shot.requested || shot.frame % 90 == 0) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            shot.requested = true;
        }
        if shot.frame > 900 {
            eprintln!("Screenshot timed out");
            std::process::exit(2);
        }
        ctx.request_repaint();
    }

    fn auto_pdf_tick(&mut self, ctx: &egui::Context) {
        let Some((path, frames)) = self.auto_pdf.clone() else {
            return;
        };
        let next = frames + 1;
        if next >= 20 {
            match self.export_pdf(ctx, &path) {
                Ok(_) => {
                    eprintln!("PDF saved to {path:?}");
                    std::process::exit(0);
                }
                Err(err) => {
                    eprintln!("PDF export failed: {err}");
                    std::process::exit(1);
                }
            }
        }
        self.auto_pdf = Some((path, next));
        ctx.request_repaint();
    }

    // ------------------------------------------------------------- chrome ---

    fn top_bar(&mut self, root_ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut actions: Vec<Action> = Vec::new();
        let can_undo = !self.undo_stack.is_empty();
        let can_redo = !self.redo_stack.is_empty();
        let can_delete = self.selected != self.doc.id;
        let title = elide(&self.doc.text, 42);
        let default_subtitle = if self.save_ok {
            "Простая карта · сохранено"
        } else {
            "Простая карта · сохранение недоступно"
        };
        let subtitle = match &self.toast {
            Some((msg, at)) if at.elapsed().as_secs_f32() < 6.0 => msg.clone(),
            _ => default_subtitle.to_owned(),
        };

        egui::Panel::top("top_bar")
            .exact_size(62.0)
            .show_separator_line(false)
            .frame(
                Frame::new()
                    .fill(WHITE)
                    .inner_margin(Margin::symmetric(18, 0)),
            )
            .show(root_ui, |ui| {
                let panel = ui.max_rect();
                ui.painter().line_segment(
                    [panel.left_bottom(), panel.right_bottom()],
                    Stroke::new(1.0, BORDER),
                );

                ui.horizontal_centered(|ui| {
                    draw_logo(ui);
                    ui.add_space(10.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(360.0, 40.0),
                        Layout::top_down(Align::LEFT),
                        |ui| {
                            ui.add_space(9.0);
                            ui.label(RichText::new(title).size(16.0).color(NAVY).strong());
                            ui.add_space(1.0);
                            ui.label(RichText::new(subtitle).size(10.5).color(MUTED));
                        },
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if pill_button(ui, "+ Ветка", true, 102.0, "Добавить ветку (Tab)")
                        {
                            actions.push(Action::AddChild);
                        }
                        if pill_button(ui, "Новая", false, 74.0, "Новая карта (Cmd+N)")
                        {
                            actions.push(Action::NewMap);
                        }
                        if pill_button(ui, "PDF", false, 60.0, "Сохранить карту в PDF (Cmd+P)")
                        {
                            actions.push(Action::ExportPdf);
                        }
                        ui.add_space(6.0);
                        let (line, _) =
                            ui.allocate_exact_size(Vec2::new(1.0, 24.0), Sense::hover());
                        ui.painter().rect_filled(line, 0, BORDER);
                        ui.add_space(6.0);
                        if icon_button(ui, Icon::Trash, can_delete, "Удалить ветку (Delete)")
                        {
                            actions.push(Action::Delete);
                        }
                        if icon_button(ui, Icon::Redo, can_redo, "Повторить (Cmd+Shift+Z)")
                        {
                            actions.push(Action::Redo);
                        }
                        if icon_button(ui, Icon::Undo, can_undo, "Отменить (Cmd+Z)") {
                            actions.push(Action::Undo);
                        }
                    });
                });
            });

        for action in actions {
            self.apply_action(action, ctx);
        }
    }

    fn bottom_bar(&mut self, root_ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut actions: Vec<Action> = Vec::new();
        let zoom = self.camera.zoom;
        let _ = ctx;

        egui::Panel::bottom("bottom_bar")
            .exact_size(38.0)
            .show_separator_line(false)
            .frame(
                Frame::new()
                    .fill(WHITE)
                    .inner_margin(Margin::symmetric(18, 0)),
            )
            .show(root_ui, |ui| {
                let panel = ui.max_rect();
                ui.painter().line_segment(
                    [panel.left_top(), panel.right_top()],
                    Stroke::new(1.0, BORDER),
                );

                ui.horizontal_centered(|ui| {
                    ui.label(
                        RichText::new(
                            "Tab — ветка · Enter — ещё одна · Delete — удалить · двойной клик — изменить · Cmd+P — PDF",
                        )
                        .size(11.0)
                        .color(MUTED),
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if pill_button(ui, "По размеру", false, 92.0, "Вписать карту в окно (F)") {
                            actions.push(Action::Fit);
                        }
                        ui.add_space(8.0);

                        let (group, _) =
                            ui.allocate_exact_size(Vec2::new(124.0, 27.0), Sense::hover());
                        ui.painter().rect_filled(group, cr(8.0), SOFT);
                        let minus_rect =
                            Rect::from_min_size(group.min, Vec2::new(29.0, group.height()));
                        let plus_rect = Rect::from_min_size(
                            Pos2::new(group.right() - 29.0, group.top()),
                            Vec2::new(29.0, group.height()),
                        );
                        let minus = ui.interact(minus_rect, Id::new("zoom_out"), Sense::click());
                        let plus = ui.interact(plus_rect, Id::new("zoom_in"), Sense::click());
                        if minus.hovered() {
                            ui.painter().rect_filled(minus_rect, cr(8.0), BORDER);
                        }
                        if plus.hovered() {
                            ui.painter().rect_filled(plus_rect, cr(8.0), BORDER);
                        }
                        let icon_color = Color32::from_rgb(83, 92, 108);
                        let mc = minus_rect.center();
                        ui.painter().line_segment(
                            [Pos2::new(mc.x - 4.5, mc.y), Pos2::new(mc.x + 4.5, mc.y)],
                            Stroke::new(1.5, icon_color),
                        );
                        let pc = plus_rect.center();
                        ui.painter().line_segment(
                            [Pos2::new(pc.x - 4.5, pc.y), Pos2::new(pc.x + 4.5, pc.y)],
                            Stroke::new(1.5, icon_color),
                        );
                        ui.painter().line_segment(
                            [Pos2::new(pc.x, pc.y - 4.5), Pos2::new(pc.x, pc.y + 4.5)],
                            Stroke::new(1.5, icon_color),
                        );
                        ui.painter().text(
                            group.center(),
                            Align2::CENTER_CENTER,
                            format!("{}%", (zoom * 100.0).round() as i32),
                            FontId::proportional(11.0),
                            TEXT,
                        );
                        if minus.clicked() {
                            actions.push(Action::ZoomOut);
                        }
                        if plus.clicked() {
                            actions.push(Action::ZoomIn);
                        }
                    });
                });
            });

        for action in actions {
            self.apply_action(action, ctx);
        }
    }
}

fn laid_font(depth: usize) -> f32 {
    match depth {
        0 => 19.0,
        1 => 15.0,
        _ => 14.0,
    }
}

fn draw_logo(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(30.0, 30.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, cr(9.0), NAVY);
    let c = rect.center() + Vec2::new(-2.0, 0.0);
    let branch = Color32::from_rgb(150, 184, 255);
    let tips = [
        c + Vec2::new(8.0, -6.5),
        c + Vec2::new(10.0, 0.0),
        c + Vec2::new(8.0, 6.5),
    ];
    for tip in tips {
        painter.line_segment([c, tip], Stroke::new(1.4, branch));
        painter.circle_filled(tip, 1.8, branch);
    }
    painter.circle_filled(c, 2.4, WHITE);
}

fn pill_button(ui: &mut egui::Ui, label: &str, primary: bool, min_width: f32, tip: &str) -> bool {
    let font = FontId::proportional(13.5);
    let color = if primary { WHITE } else { TEXT };
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, color);
    let width = (galley.size().x + 30.0).max(min_width);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 34.0), Sense::click());
    let response = response.on_hover_text(tip);
    let (fill, hover_fill) = if primary {
        (NAVY, NAVY_HOVER)
    } else {
        (Color32::TRANSPARENT, SOFT)
    };
    let fill = if response.hovered() { hover_fill } else { fill };
    ui.painter().rect_filled(rect, cr(10.0), fill);
    if !primary && response.hovered() {
        ui.painter()
            .rect_stroke(rect, cr(10.0), Stroke::new(1.0, BORDER), StrokeKind::Inside);
    }
    ui.painter()
        .galley(rect.center() - galley.size() * 0.5, galley, color);
    response.clicked()
}

fn icon_button(ui: &mut egui::Ui, icon: Icon, enabled: bool, tip: &str) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(34.0, 34.0), Sense::click());
    let response = response.on_hover_text(tip);
    if response.hovered() && enabled {
        ui.painter().rect_filled(rect, cr(9.0), SOFT);
    }
    let color = if enabled {
        Color32::from_rgb(78, 86, 101)
    } else {
        Color32::from_rgb(194, 199, 208)
    };
    draw_icon(ui.painter(), rect, icon, color);
    response.clicked() && enabled
}

fn draw_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let stroke = Stroke::new(1.7, color);
    match icon {
        Icon::Undo | Icon::Redo => {
            let mirror = if matches!(icon, Icon::Redo) {
                -1.0
            } else {
                1.0
            };
            let center = Pos2::new(c.x + 0.5 * mirror, c.y + 1.0);
            let radius = 6.2;
            let mut points = Vec::new();
            let steps = 22;
            for i in 0..=steps {
                let a = 3.72 + (5.05 * i as f32 / steps as f32);
                points.push(center + Vec2::new(a.cos() * mirror, a.sin()) * radius);
            }
            painter.add(egui::Shape::line(points.clone(), stroke));
            let tip = points[0];
            let next = points[1];
            let dir = (next - tip).normalized();
            let perp = Vec2::new(-dir.y, dir.x);
            painter.add(egui::Shape::convex_polygon(
                vec![tip + dir * 3.6, tip + perp * 2.5, tip - perp * 2.5],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Trash => {
            let lid_y = c.y - 5.5;
            painter.line_segment(
                [Pos2::new(c.x - 7.5, lid_y), Pos2::new(c.x + 7.5, lid_y)],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x - 2.2, lid_y - 3.2),
                    Pos2::new(c.x + 2.2, lid_y - 3.2),
                ],
                stroke,
            );
            let body = Rect::from_min_max(
                Pos2::new(c.x - 5.5, lid_y + 2.0),
                Pos2::new(c.x + 5.5, c.y + 8.0),
            );
            painter.rect_stroke(body, cr(2.0), stroke, StrokeKind::Inside);
            painter.line_segment(
                [
                    Pos2::new(c.x - 2.0, lid_y + 4.5),
                    Pos2::new(c.x - 2.0, c.y + 5.5),
                ],
                Stroke::new(1.3, color),
            );
            painter.line_segment(
                [
                    Pos2::new(c.x + 2.0, lid_y + 4.5),
                    Pos2::new(c.x + 2.0, c.y + 5.5),
                ],
                Stroke::new(1.3, color),
            );
        }
    }
}

impl eframe::App for MindMapApp {
    fn ui(&mut self, root_ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root_ui.ctx().clone();
        self.handle_screenshot_event(&ctx);
        self.handle_keys(&ctx);
        self.top_bar(root_ui, &ctx);
        self.bottom_bar(root_ui, &ctx);

        egui::CentralPanel::default()
            .frame(Frame::new().fill(CANVAS))
            .show(root_ui, |ui| {
                self.canvas_ui(ui, &ctx);
            });

        self.screenshot_tick(&ctx);
        self.auto_pdf_tick(&ctx);
    }
}

fn main() -> eframe::Result {
    let shot = parse_screenshot_arg();
    let pdf = parse_pdf_arg();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([760.0, 500.0])
            .with_title("Карта мыслей"),
        ..Default::default()
    };

    eframe::run_native(
        "Карта мыслей",
        options,
        Box::new(move |cc| Ok(Box::new(MindMapApp::new(cc, shot, pdf)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_ids(node: &Node, out: &mut Vec<u64>) {
        out.push(node.id);
        for child in &node.children {
            collect_ids(child, out);
        }
    }

    #[test]
    fn sample_has_unique_ids() {
        let mut ids = Vec::new();
        collect_ids(&sample_map(), &mut ids);
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len());
        assert_eq!(max_id(&sample_map()) + 1, 15);
    }

    #[test]
    fn detach_and_reparent_works() {
        let mut map = sample_map();
        let detached = detach(&mut map, 5).expect("child exists");
        assert_eq!(detached.text, "Новая привычка");
        assert!(find(&map, 5).is_none());
        find_mut(&mut map, 3).unwrap().children.push(detached);
        assert_eq!(parent_of(&map, 5), Some(3));
        assert!(contains_id(find(&map, 2).unwrap(), 5) == false);
    }

    #[test]
    fn serde_roundtrip_keeps_tree() {
        let map = sample_map();
        let text = serde_json::to_string(&map).unwrap();
        let restored: Node = serde_json::from_str(&text).unwrap();
        assert_eq!(restored.text, "Моя карта");
        assert_eq!(restored.children.len(), 4);
        assert_eq!(max_id(&restored), 14);
    }
}
