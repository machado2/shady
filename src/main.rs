use std::sync::Arc;
use std::time::Instant;

use eframe::{egui, egui_glow, glow};
use egui::mutex::Mutex;

const DEFAULT_SNIPPET: &str = r"vec2 p=FC.xy*6./r.y;
for(float i;i++<1e1;i)
    p+=sin(p.yx*i+i*i+t*i+r)/i;
o=tanh(.2/tan(p.y+vec4(0.,.1,.3,0.)));
o*=o;";

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
            out vec4 fragColor;

            void main() {{
                vec2 FC = gl_FragCoord.xy;
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

    fn paint(&self, gl: &glow::Context, time: f32, resolution: egui::Vec2) {
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

            gl.bind_vertex_array(Some(self.vertex_array));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
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

struct ShadyApp {
    gl: Arc<glow::Context>,
    snippet: String,
    last_error: Option<String>,
    shader: Option<Arc<Mutex<ShaderState>>>,
    start_time: Instant,
    needs_recompile: bool,
}

impl ShadyApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let gl = cc
            .gl
            .as_ref()
            .expect("You need to run eframe with the glow backend")
            .clone();

        let mut this = Self {
            gl,
            snippet: DEFAULT_SNIPPET.to_owned(),
            last_error: None,
            shader: None,
            start_time: Instant::now(),
            needs_recompile: true,
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
}

impl eframe::App for ShadyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.needs_recompile {
            self.recompile();
        }

        egui::TopBottomPanel::top("controls").show(ctx, |ui| {
            ui.heading("Shady - GLSL tweet shader");

            ui.label("Paste a GLSL snippet that writes to `o` (vec4), using FC, r, t.");

            let edit = egui::TextEdit::multiline(&mut self.snippet)
                .desired_rows(6)
                .hint_text("vec2 p=FC.xy*6./r.y;\nfor(float i;i++<1e1;i)\n    p+=sin(p.yx*i+i*i+t*i+r)/i;\no=tanh(.2/tan(p.y+vec4(0.,.1,.3,0.)));\no*=o;");

            let response = ui.add(edit);

            if response.changed() {
                self.needs_recompile = true;
            }

            if let Some(err) = &self.last_error {
                ui.colored_label(egui::Color32::RED, err);
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::Frame::canvas(ui.style()).show(ui, |ui| {
                let available = ui.available_size();
                let (rect, _response) =
                    ui.allocate_exact_size(available, egui::Sense::hover());

                let time = self.start_time.elapsed().as_secs_f32();

                if let Some(shader) = &self.shader {
                    let shader = shader.clone();
                    let resolution = rect.size();

                    let callback = egui::PaintCallback {
                        rect,
                        callback: Arc::new(egui_glow::CallbackFn::new(
                            move |_info, painter| {
                                let gl = painter.gl();
                                shader.lock().paint(gl, time, resolution);
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

