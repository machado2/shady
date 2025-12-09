use std::fs::File;
use std::io::BufWriter;
use std::sync::Arc;
use std::time::Instant;

use eframe::{egui, egui_glow, glow};
use egui::mutex::Mutex;
use gif::{Encoder as GifEncoder, Frame as GifFrame, Repeat};

const DEFAULT_SNIPPET: &str = r"// Simple radial swirl
vec2 uv = (FC - r * vec2(0.7, 0.5)) / r.y * 2.0;
float d = length(uv);
float angle = atan(uv.y, uv.x);
float v = 0.5 + 0.5 * sin(8.0 * d - 2.0 * t + 4.0 * angle);
o = vec4(vec3(v), 1.0);";

struct ShaderState {
    program: glow::Program,
    vertex_array: glow::VertexArray,
}

impl ShaderState {
    fn new(gl: &glow::Context, snippet: &str) -> Result<Self, String> {
        use glow::HasContext as _;

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

        let fragment_body = format!(
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

        let fragment_shader_source = format!("{shader_version}\n{fragment_body}");

        unsafe {
            let program = gl
                .create_program()
                .map_err(|e| format!("Cannot create program: {e}"))?;

            let vs = compile_shader(gl, glow::VERTEX_SHADER, &vertex_shader_source)
                .map_err(|e| {
                    gl.delete_program(program);
                    e
                })?;
            let fs = compile_shader(gl, glow::FRAGMENT_SHADER, &fragment_shader_source)
                .map_err(|e| {
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

            Ok(Self {
                program,
                vertex_array,
            })
        }
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
}

impl ShadyApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let gl = cc
            .gl
            .as_ref()
            .expect("You need to run eframe with the glow backend")
            .clone();

        // Tweak global egui style for a more polished look.
        let ctx = &cc.egui_ctx;
        let mut style: egui::Style = (*ctx.style()).clone();

        // Typography
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(20.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(14.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(14.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            egui::FontId::new(14.0, egui::FontFamily::Monospace),
        );

        // Subtle light theme with softer panel background.
        let mut visuals = style.visuals.clone();
        visuals.panel_fill = egui::Color32::from_rgb(247, 248, 252);
        visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(220, 224, 235),
        );
        style.visuals = visuals;

        ctx.set_style(style);

        let mut this = Self {
            gl,
            snippet: DEFAULT_SNIPPET.to_owned(),
            last_error: None,
            shader: None,
            start_time: Instant::now(),
            needs_recompile: true,
            gif_export: None,
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
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            egui::Frame::none()
                .fill(ui.visuals().panel_fill)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new("Shady")
                                    .heading()
                                    .strong(),
                            );
                            ui.label(
                                egui::RichText::new("GLSL tweet shader playground")
                                    .small()
                                    .color(ui.visuals().weak_text_color()),
                            );
                        });

                        ui.add_space(16.0);

                        if ui.button("Export GIF").clicked() {
                            self.start_gif_export();
                        }

                        if let Some(export) = &self.gif_export {
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "Exporting GIF: {}/{}",
                                    export.frame_index, export.frame_count
                                ))
                                .small()
                                .color(ui.visuals().weak_text_color()),
                            );
                        }

                        if self.last_error.is_some() {
                            ui.add_space(12.0);
                            ui.colored_label(
                                egui::Color32::from_rgb(220, 30, 60),
                                "Shader error",
                            );
                        } else if self.needs_recompile {
                            ui.add_space(12.0);
                            ui.label(
                                egui::RichText::new("Recompiling…")
                                    .italics()
                                    .color(ui.visuals().weak_text_color()),
                            );
                        }

                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                let secs = self.start_time.elapsed().as_secs_f32();
                                ui.label(
                                    egui::RichText::new(format!("t = {:.1}s", secs))
                                        .monospace()
                                        .color(ui.visuals().weak_text_color()),
                                );
                            },
                        );
                    });
                });
        });

        egui::SidePanel::left("code_panel")
            .resizable(true)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("Shader editor")
                            .heading()
                            .strong(),
                    );
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new("Writes to `o` (vec4). Vars: FC, r, t.")
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );
                    ui.add_space(8.0);

                    egui::Frame::group(ui.style())
                        .fill(ui.visuals().extreme_bg_color)
                        .rounding(egui::Rounding::same(6))
                        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_min_height(ui.available_height());

                            let edit = egui::TextEdit::multiline(&mut self.snippet)
                                .font(egui::TextStyle::Monospace)
                                .desired_rows(18)
                                .desired_width(f32::INFINITY);

                            let response = ui.add(edit);

                            if response.changed() {
                                self.needs_recompile = true;
                            }

                            if let Some(err) = &self.last_error {
                                ui.add_space(6.0);
                                ui.colored_label(egui::Color32::RED, err);
                            }
                        });
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::Frame::canvas(ui.style()).show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("Preview")
                            .heading()
                            .strong(),
                    );
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new("Live shader output")
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );
                    ui.add_space(8.0);

                    egui::Frame::group(ui.style())
                        .fill(egui::Color32::BLACK)
                        .rounding(egui::Rounding::same(8))
                        .inner_margin(egui::Margin::symmetric(8, 8))
                        .show(ui, |ui| {
                            let available = ui.available_size();
                            let side = available.x.min(available.y);
                            let size = egui::vec2(side, side);
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
                                            shader
                                                .lock()
                                                .paint(gl, time, rect_min, resolution);
                                        },
                                    )),
                                };
                                ui.painter().add(callback);
                            } else {
                                ui.painter()
                                    .rect_filled(rect, 0.0, egui::Color32::BLACK);
                            }
                        });
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

