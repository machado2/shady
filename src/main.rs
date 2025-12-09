use std::fs::{self, File};
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use eframe::{egui, egui_glow, glow};
use egui::mutex::Mutex;
use egui_code_editor::{CodeEditor, ColorTheme, Syntax};
use gif::{Encoder as GifEncoder, Frame as GifFrame, Repeat};
use rfd::FileDialog;

const DEFAULT_SNIPPET: &str = r"// Colorful warped waves
vec2 uv = FC.xy / r.xy;
uv.x *= r.x / r.y;

float time = t * 0.5;

for (float i = 1.0; i < 4.0; i++) {
    uv.x += 0.6 / i * cos(i * 2.5 * uv.y + time);
    uv.y += 0.6 / i * cos(i * 1.5 * uv.x + time);
}

vec3 color = vec3(0.0);
color.r = 0.5 + 0.5 * sin(uv.x + time);
color.g = 0.5 + 0.5 * sin(uv.y + time + 2.0);
color.b = 0.5 + 0.5 * sin(uv.x + uv.y + time + 4.0);

o = vec4(color, 1.0);";

struct ShaderState {
    program: glow::Program,
    vertex_array: glow::VertexArray,
}

impl ShaderState {
    fn new(gl: &glow::Context, snippet: &str) -> Result<Self, String> {
        let (shader_version, precision_line) = if cfg!(target_arch = "wasm32") {
            ("#version 300 es", "precision mediump float;")
        } else {
            ("#version 330 core", "")
        };

        let vertex_shader_source = format!(
            "{shader_version}\n{}",
            r#"
            const vec2 verts[3] = vec2[3](
                vec2(-1.0, -1.0),
                vec2(3.0, -1.0),
                vec2(-1.0, 3.0)
            );

            void main() {
                gl_Position = vec4(verts[gl_VertexID], 0.0, 1.0);
            }
        "#
        );
        // Build both variants up front.
        // Tweet-style body that writes to `o` and uses FC, r, t.
        let tweet_fragment_body = format!(
            r#"
            {precision_line}
            uniform vec2 r;
            uniform float t;
            uniform vec2 rect_min;
            out vec4 fragColor;

            void main() {{
                vec2 FC = gl_FragCoord.xy - rect_min;
                vec4 o = vec4(0.0);
                {snippet}
                fragColor = o;
            }}
        "#
        );

        let tweet_fragment_source = format!("{shader_version}\n{tweet_fragment_body}");

        // Full GLSL fragment shader variant.
        let full_fragment_source = if snippet.contains("#version") {
            snippet.to_owned()
        } else if precision_line.is_empty() {
            format!("{shader_version}\n{snippet}")
        } else {
            format!("{shader_version}\n{precision_line}\n{snippet}")
        };

        // Heuristic: if the snippet looks like a complete GLSL shader (has
        // `void main`, `#version`, or explicit outputs), try full mode first;
        // otherwise prefer tweet mode first. On failure, fall back to the other
        // mode.
        let looks_like_full = {
            let s = snippet;
            s.contains("void main")
                || s.contains("#version")
                || s.contains("gl_FragColor")
                || s.contains("out vec4")
        };

        unsafe {
            if looks_like_full {
                match Self::create_program(gl, &vertex_shader_source, &full_fragment_source) {
                    Ok(state) => Ok(state),
                    Err(full_err) => match Self::create_program(
                        gl,
                        &vertex_shader_source,
                        &tweet_fragment_source,
                    ) {
                        Ok(state) => Ok(state),
                        Err(tweet_err) => Err(format!(
                            "Full GLSL mode failed:\n{}\n\nTweet shader mode also failed:\n{}",
                            full_err, tweet_err
                        )),
                    },
                }
            } else {
                match Self::create_program(gl, &vertex_shader_source, &tweet_fragment_source) {
                    Ok(state) => Ok(state),
                    Err(tweet_err) => match Self::create_program(
                        gl,
                        &vertex_shader_source,
                        &full_fragment_source,
                    ) {
                        Ok(state) => Ok(state),
                        Err(full_err) => Err(format!(
                            "Tweet shader mode failed:\n{}\n\nFull GLSL mode also failed:\n{}",
                            tweet_err, full_err
                        )),
                    },
                }
            }
        }
    }

    unsafe fn create_program(
        gl: &glow::Context,
        vertex_shader_source: &str,
        fragment_shader_source: &str,
    ) -> Result<Self, String> {
        use glow::HasContext as _;

        let program = gl
            .create_program()
            .map_err(|e| format!("Cannot create program: {e}"))?;

        let vs = compile_shader(gl, glow::VERTEX_SHADER, vertex_shader_source).map_err(|e| {
            gl.delete_program(program);
            e
        })?;
        let fs = compile_shader(gl, glow::FRAGMENT_SHADER, fragment_shader_source).map_err(|e| {
            gl.delete_shader(vs);
            gl.delete_program(program);
            e
        })?;

        gl.attach_shader(program, vs);
        gl.attach_shader(program, fs);

        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            gl.delete_shader(vs);
            gl.delete_shader(fs);
            gl.delete_program(program);
            return Err(format!("Program link error:\n{log}"));
        }

        gl.detach_shader(program, vs);
        gl.detach_shader(program, fs);
        gl.delete_shader(vs);
        gl.delete_shader(fs);

        let vertex_array = gl
            .create_vertex_array()
            .map_err(|e| format!("Cannot create vertex array: {e}"))?;

        Ok(Self { program, vertex_array })
    }

    fn paint(
        &self,
        gl: &glow::Context,
        time: f32,
        rect_min: egui::Pos2,
        resolution: egui::Vec2,
    ) {
        use glow::HasContext as _;
        unsafe {
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);

            gl.use_program(Some(self.program));

            if let Some(loc) = gl.get_uniform_location(self.program, "t") {
                gl.uniform_1_f32(Some(&loc), time);
            }
            if let Some(loc) = gl.get_uniform_location(self.program, "r") {
                gl.uniform_2_f32(Some(&loc), resolution.x, resolution.y);
            }
            if let Some(loc) = gl.get_uniform_location(self.program, "rect_min") {
                gl.uniform_2_f32(Some(&loc), rect_min.x, rect_min.y);
            }

            gl.bind_vertex_array(Some(self.vertex_array));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
        }
    }

    fn render_to_image(
        &self,
        gl: &glow::Context,
        time: f32,
        size: [u32; 2],
    ) -> Result<Vec<u8>, String> {
        use glow::HasContext as _;

        let width = size[0] as i32;
        let height = size[1] as i32;

        unsafe {
            let framebuffer = gl
                .create_framebuffer()
                .map_err(|e| format!("Failed to create framebuffer: {e}"))?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));

            let texture = gl
                .create_texture()
                .map_err(|e| format!("Failed to create texture: {e}"))?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                width,
                height,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::BufferOffset(0),
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );

            if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.delete_texture(texture);
                gl.delete_framebuffer(framebuffer);
                return Err("Framebuffer is not complete".to_owned());
            }

            gl.viewport(0, 0, width, height);

            self.paint(
                gl,
                time,
                egui::Pos2::new(0.0, 0.0),
                egui::vec2(width as f32, height as f32),
            );

            let mut pixels = vec![0u8; (width * height * 4) as usize];
            gl.read_pixels(
                0,
                0,
                width,
                height,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(pixels.as_mut_slice())),
            );

            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.delete_texture(texture);
            gl.delete_framebuffer(framebuffer);

            Ok(pixels)
        }
    }
}

unsafe fn compile_shader(
    gl: &glow::Context,
    shader_type: u32,
    source: &str,
) -> Result<glow::Shader, String> {
    use glow::HasContext as _;
    let shader = gl
        .create_shader(shader_type)
        .map_err(|e| format!("Cannot create shader: {e}"))?;
    gl.shader_source(shader, source);
    gl.compile_shader(shader);
    if !gl.get_shader_compile_status(shader) {
        let log = gl.get_shader_info_log(shader);
        gl.delete_shader(shader);
        Err(format!("Shader compile error:\n{log}"))
    } else {
        Ok(shader)
    }
}

struct GifExportState {
    encoder: GifEncoder<BufWriter<File>>,
    shader: Arc<Mutex<ShaderState>>,
    frame_index: u32,
    frame_count: u32,
    width: u32,
    height: u32,
    fps: u32,
}

struct ShadyApp {
    gl: Arc<glow::Context>,
    snippet: String,
    last_error: Option<String>,
    shader: Option<Arc<Mutex<ShaderState>>>,
    start_time: Instant,
    needs_recompile: bool,
    gif_export: Option<GifExportState>,
    current_file: Option<PathBuf>,
    is_dirty: bool,
}

impl ShadyApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let gl = cc
            .gl
            .as_ref()
            .expect("You need to run eframe with the glow backend")
            .clone();

        let ctx = &cc.egui_ctx;
        let mut style: egui::Style = (*ctx.style()).clone();

        // Modern typography
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(16.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            egui::FontId::new(13.0, egui::FontFamily::Monospace),
        );

        // Modern dark theme
        let mut visuals = egui::Visuals::dark();
        
        // Background colors
        let bg_dark = egui::Color32::from_rgb(17, 17, 21);
        let bg_medium = egui::Color32::from_rgb(24, 24, 30);
        let bg_light = egui::Color32::from_rgb(32, 32, 40);
        let border = egui::Color32::from_rgb(45, 45, 55);
        let accent = egui::Color32::from_rgb(99, 102, 241); // Indigo
        let accent_hover = egui::Color32::from_rgb(129, 132, 255);
        let text_primary = egui::Color32::from_rgb(240, 240, 245);
        let text_muted = egui::Color32::from_rgb(140, 140, 160);
        
        visuals.panel_fill = bg_dark;
        visuals.window_fill = bg_medium;
        visuals.extreme_bg_color = bg_medium;
        visuals.faint_bg_color = bg_light;
        
        // Widget styling
        visuals.widgets.noninteractive.bg_fill = bg_medium;
        visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, border);
        visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, text_muted);
        
        visuals.widgets.inactive.bg_fill = bg_light;
        visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, border);
        visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, text_primary);
        visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
        
        visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(50, 50, 65);
        visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, accent);
        visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, text_primary);
        visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
        
        visuals.widgets.active.bg_fill = accent;
        visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, accent_hover);
        visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, text_primary);
        visuals.widgets.active.corner_radius = egui::CornerRadius::same(6);
        
        visuals.selection.bg_fill = accent.linear_multiply(0.4);
        visuals.selection.stroke = egui::Stroke::new(1.0, accent);
        
        visuals.window_corner_radius = egui::CornerRadius::same(8);
        visuals.window_stroke = egui::Stroke::new(1.0, border);
        
        style.visuals = visuals;
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);

        let mut scroll = style.spacing.scroll.clone();
        scroll.bar_width = 12.0;
        scroll.handle_min_length = 40.0;
        scroll.floating = false;
        style.spacing.scroll = scroll;

        ctx.set_style(style);

        let mut this = Self {
            gl,
            snippet: DEFAULT_SNIPPET.to_owned(),
            last_error: None,
            shader: None,
            start_time: Instant::now(),
            needs_recompile: true,
            gif_export: None,
            current_file: None,
            is_dirty: false,
        };

        this.recompile();
        this
    }

    fn recompile(&mut self) {
        match ShaderState::new(&self.gl, &self.snippet) {
            Ok(new_shader) => {
                self.shader = Some(Arc::new(Mutex::new(new_shader)));
                self.last_error = None;
            }
            Err(err) => {
                self.shader = None;
                self.last_error = Some(err);
            }
        }
        self.needs_recompile = false;
    }

    fn start_gif_export(&mut self) {
        if self.gif_export.is_some() {
            return;
        }

        let shader = match &self.shader {
            Some(shader) => shader.clone(),
            None => {
                self.last_error = Some("No compiled shader to export".to_owned());
                return;
            }
        };

        let width = 512u32;
        let height = 512u32;
        let fps = 30u32;
        let seconds = 3u32;
        let frame_count = fps * seconds;

        let file = match File::create("shady_export.gif") {
            Ok(file) => file,
            Err(e) => {
                self.last_error = Some(format!("Failed to create GIF file: {e}"));
                return;
            }
        };
        let writer = BufWriter::new(file);

        let mut encoder = match GifEncoder::new(writer, width as u16, height as u16, &[]) {
            Ok(encoder) => encoder,
            Err(e) => {
                self.last_error = Some(format!("Failed to create GIF encoder: {e}"));
                return;
            }
        };

        if let Err(e) = encoder.set_repeat(Repeat::Infinite) {
            self.last_error = Some(format!("Failed to set GIF repeat: {e}"));
            return;
        }

        self.gif_export = Some(GifExportState {
            encoder,
            shader,
            frame_index: 0,
            frame_count,
            width,
            height,
            fps,
        });
    }

    fn step_gif_export(&mut self) {
        let Some(export) = self.gif_export.as_mut() else {
            return;
        };

        if export.frame_index >= export.frame_count {
            self.gif_export = None;
            return;
        }

        let result: Result<(), String> = (|| {
            let t = export.frame_index as f32 / export.fps as f32;

            let mut rgba = export
                .shader
                .lock()
                .render_to_image(&self.gl, t, [export.width, export.height])?;
            let rgba_slice = rgba.as_mut_slice();

            let mut frame = GifFrame::from_rgba_speed(
                export.width as u16,
                export.height as u16,
                rgba_slice,
                10,
            );
            frame.delay = (100 / export.fps) as u16;

            export
                .encoder
                .write_frame(&frame)
                .map_err(|e| format!("Failed to write GIF frame: {e}"))?;

            Ok(())
        })();

        match result {
            Ok(()) => {
                export.frame_index += 1;
                if export.frame_index >= export.frame_count {
                    self.gif_export = None;
                }
            }
            Err(err) => {
                self.last_error = Some(err);
                self.gif_export = None;
            }
        }
    }
}

impl eframe::App for ShadyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.needs_recompile {
            self.recompile();
        }

        let accent = egui::Color32::from_rgb(99, 102, 241);
        let success_color = egui::Color32::from_rgb(34, 197, 94);
        let error_color = egui::Color32::from_rgb(239, 68, 68);
        let border_color = egui::Color32::from_rgb(45, 45, 55);

        // Minimal top toolbar
        egui::TopBottomPanel::top("top_bar")
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(20, 20, 26))
                    .inner_margin(egui::Margin::symmetric(16, 10))
                    .stroke(egui::Stroke::new(1.0, border_color)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // Logo with accent color
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("Shady")
                                .strong()
                                .size(16.0)
                                .color(egui::Color32::WHITE),
                        );
                        ui.label(
                            egui::RichText::new("GLSL tweet shader playground")
                                .size(11.0)
                                .color(egui::Color32::from_rgb(160, 160, 180)),
                        );
                    });

                    ui.add_space(12.0);

                    // Status indicator dot with tooltip
                    let (status_color, status_tip) = if self.last_error.is_some() {
                        (error_color, "Shader has errors")
                    } else if self.gif_export.is_some() {
                        (accent, "Exporting GIF...")
                    } else {
                        (success_color, "Shader compiled")
                    };

                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 4.0, status_color);
                    resp.on_hover_text(status_tip);

                    ui.add_space(16.0);

                    // Export button
                    let export_btn = egui::Button::new(
                        egui::RichText::new(" Export GIF").size(12.0),
                    );
                    if ui
                        .add_enabled(self.gif_export.is_none(), export_btn)
                        .clicked()
                    {
                        self.start_gif_export();
                    }

                    ui.add_space(16.0);

                    // File open/save
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(" Open").size(12.0),
                            ),
                        )
                        .clicked()
                    {
                        if let Some(path) = FileDialog::new()
                            .add_filter("GLSL", &["glsl", "frag"])
                            .pick_file()
                        {
                            match fs::read_to_string(&path) {
                                Ok(contents) => {
                                    self.snippet = contents;
                                    self.current_file = Some(path);
                                    self.is_dirty = false;
                                    self.needs_recompile = true;
                                    self.last_error = None;
                                }
                                Err(e) => {
                                    self.last_error =
                                        Some(format!("Failed to load file: {e}"));
                                }
                            }
                        }
                    }

                    let save_label = if self.current_file.is_some() {
                        " Save"
                    } else {
                        " Save As"
                    };
                    if ui
                        .add_enabled(
                            !self.snippet.is_empty(),
                            egui::Button::new(
                                egui::RichText::new(save_label).size(12.0),
                            ),
                        )
                        .clicked()
                    {
                        let target_path = if let Some(path) = &self.current_file {
                            Some(path.clone())
                        } else {
                            FileDialog::new()
                                .set_file_name("shader.glsl")
                                .add_filter("GLSL", &["glsl", "frag"])
                                .save_file()
                        };

                        if let Some(path) = target_path {
                            match fs::write(&path, &self.snippet) {
                                Ok(()) => {
                                    self.current_file = Some(path);
                                    self.is_dirty = false;
                                }
                                Err(e) => {
                                    self.last_error =
                                        Some(format!("Failed to save file: {e}"));
                                }
                            }
                        }
                    }

                    // Right side: time display + reset
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let secs = self.start_time.elapsed().as_secs_f32();

                        // Reset time button
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new("↺").size(14.0),
                            ))
                            .on_hover_text("Reset time")
                            .clicked()
                        {
                            self.start_time = Instant::now();
                        }

                        ui.add_space(4.0);

                        ui.label(
                            egui::RichText::new(format!("{:.1}s", secs))
                                .monospace()
                                .size(13.0)
                                .color(accent),
                        );

                        ui.label(
                            egui::RichText::new("t =")
                                .monospace()
                                .size(13.0)
                                .color(egui::Color32::from_rgb(140, 140, 160)),
                        );
                    });
                });
            });

        // Code editor panel
        egui::SidePanel::left("code_panel")
            .resizable(true)
            .default_width(380.0)
            .min_width(280.0)
            .show_separator_line(true)
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(17, 17, 21))
                    .inner_margin(egui::Margin {
                        left: 12,
                        right: 12,
                        top: 12,
                        bottom: 12,
                    })
                    .stroke(egui::Stroke::NONE),
            )
            .show(ctx, |ui| {
                // Minimal header with hint on hover
                ui.horizontal(|ui| {
                    let file_name = self
                        .current_file
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                        .unwrap_or("untitled.glsl");
                    let mut label = file_name.to_owned();
                    if self.is_dirty {
                        label.push_str("  modified");
                    }
                    ui.label(
                        egui::RichText::new(label)
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(180, 180, 200)),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("o: vec4 • FC, r, t")
                                .size(10.0)
                                .color(egui::Color32::from_rgb(90, 90, 110)),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("GLSL")
                                .size(11.0)
                                .color(egui::Color32::from_rgb(140, 140, 160)),
                        )
                        .on_hover_text(
                            "Output: o (vec4)\nInputs: FC (fragCoord), r (resolution), t (time)",
                        );
                    });
                });

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(8.0);

                // Code editor with custom styling
                let editor_height = ui.available_height().max(220.0);
                let editor_frame = egui::Frame::group(ui.style())
                    .fill(egui::Color32::from_rgb(13, 13, 17))
                    .corner_radius(egui::CornerRadius::same(6))
                    .stroke(egui::Stroke::new(1.0, border_color))
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_min_height(editor_height);

                        let before = self.snippet.clone();

                        let response = CodeEditor::default()
                            .id_source("shader_code_editor")
                            .with_rows(18)
                            .with_fontsize(14.0)
                            .with_theme(ColorTheme::GRUVBOX)
                            .with_syntax(Syntax::rust())
                            .with_numlines(true)
                            .vscroll(true)
                            .show(ui, &mut self.snippet);

                        if self.snippet != before {
                            self.needs_recompile = true;
                            self.is_dirty = true;
                        }

                        response
                    });

                // Custom focus border around the whole editor card
                if editor_frame.inner.response.has_focus() {
                    // Draw just inside the frame so it is never clipped on the right
                    let rect = editor_frame.response.rect.shrink(1.0);
                    ui.painter().rect_stroke(
                        rect,
                        egui::CornerRadius::same(6),
                        egui::Stroke::new(1.3, accent),
                        egui::StrokeKind::Inside,
                    );
                }
            });

        // Preview panel (main area)
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(8, 8, 12))
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ctx, |ui| {
                // Full bleed preview - shader fills the panel
                let available = ui.available_size();
                let side = (available.x.min(available.y) - 32.0).max(150.0);
                let size = egui::vec2(side, side);

                ui.centered_and_justified(|ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::BLACK)
                        .corner_radius(egui::CornerRadius::same(10))
                        .stroke(egui::Stroke::new(1.0, border_color))
                        .shadow(egui::epaint::Shadow {
                            offset: [0, 4],
                            blur: 18,
                            spread: 0,
                            color: egui::Color32::from_rgba_unmultiplied(0, 0, 0, 120),
                        })
                        .show(ui, |ui| {
                            let (rect, _response) =
                                ui.allocate_exact_size(size, egui::Sense::hover());

                            let time = self.start_time.elapsed().as_secs_f32();

                            if let Some(shader) = &self.shader {
                                let shader = shader.clone();
                                let resolution = rect.size();
                                let rect_min = rect.min;

                                let callback = egui::PaintCallback {
                                    rect,
                                    callback: Arc::new(egui_glow::CallbackFn::new(
                                        move |_info, painter| {
                                            let gl = painter.gl();
                                            shader.lock().paint(gl, time, rect_min, resolution);
                                        },
                                    )),
                                };
                                ui.painter().add(callback);
                            } else if let Some(err) = &self.last_error {
                                let mut err_ui = ui.new_child(
                                    egui::UiBuilder::new()
                                        .max_rect(rect)
                                        .layout(egui::Layout::top_down(
                                            egui::Align::Center,
                                        )),
                                );
                                err_ui.painter().rect_filled(
                                    err_ui.max_rect(),
                                    8.0,
                                    error_color.linear_multiply(0.22),
                                );
                                egui::ScrollArea::both()
                                    .auto_shrink([false; 2])
                                    .show(&mut err_ui, |ui| {
                                        ui.vertical_centered(|ui| {
                                            ui.label(
                                                egui::RichText::new("⚠ Shader error")
                                                    .strong()
                                                    .color(egui::Color32::from_rgb(
                                                        255, 220, 220,
                                                    ))
                                                    .size(13.0),
                                            );
                                            ui.add_space(6.0);
                                            ui.label(
                                                egui::RichText::new(err)
                                                    .monospace()
                                                    .size(12.0)
                                                    .color(egui::Color32::from_rgb(
                                                        250, 200, 200,
                                                    ))
                                                    .line_height(Some(16.0)),
                                            );
                                        });
                                    });
                            } else {
                                ui.painter()
                                    .rect_filled(rect, 8.0, egui::Color32::BLACK);
                            }
                        });
                });
            });

        self.step_gif_export();

        ctx.request_repaint();
    }
}

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "Shady - GLSL tweet shader",
        native_options,
        Box::new(|cc| Ok(Box::new(ShadyApp::new(cc)))),
    )
}

