use bytemuck::{Pod, Zeroable};
use std::{ops::Deref, sync::Arc};
use vulkano::{
    buffer::{BufferUsage, CpuAccessibleBuffer, TypedBufferAccess},
    command_buffer::{
        AutoCommandBufferBuilder, CommandBufferUsage, RenderPassBeginInfo, SubpassContents,
    },
    device::{
        physical::PhysicalDevice, Device, DeviceCreateInfo, DeviceExtensions, Features,
        QueueCreateInfo,
    },
    format::ClearValue,
    image::{view::ImageView, ImageAccess, ImageUsage},
    impl_vertex,
    instance::{
        self,
        debug::{
            DebugUtilsMessageSeverity, DebugUtilsMessageType, DebugUtilsMessenger,
            DebugUtilsMessengerCreateInfo,
        },
        Instance, InstanceCreateInfo, InstanceExtensions,
    },
    pipeline::{
        graphics::{
            rasterization::RasterizationState,
            vertex_input::BuffersDefinition,
            viewport::{Viewport, ViewportState},
        },
        GraphicsPipeline,
    },
    render_pass::{Framebuffer, FramebufferCreateInfo, Subpass},
    single_pass_renderpass,
    swapchain::{acquire_next_image, Swapchain, SwapchainCreateInfo},
    sync::GpuFuture,
    Version,
};
use vulkano_win::VkSurfaceBuild;
use winit::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};

fn main() {
    //有効にできそうなレイヤの配列
    let validation_layers = [
        "VK_LAYER_LUNARG_standard_validation",
        "VK_LAYER_KHRONOS_validation",
    ];

    //使いたいレイヤで使用可能なのリストを取得->使用できるもの、かつ使いたいものをベクタに入れる
    let enabled_layers: Vec<_> = instance::layers_list()
        .unwrap()
        .filter_map(|layer| {
            if validation_layers.contains(&layer.name()) {
                Some(layer.name().to_string())
            } else {
                None
            }
        })
        .collect();

    //インスタンスで有効にする拡張機能。デバッグのやつ入れた。
    let extensions = InstanceExtensions {
        ext_debug_utils: true,
        // FIXME: khr_win32_surface: true, Windowsでは必須だが残念ながらどうサポートされているExtensionを取得できるかわからない。
        khr_surface: true,
        ext_debug_report: true,
        ..vulkano_win::required_extensions()
    };

    //構造体にインスタンスの情報を書いてそれを渡して作る。
    let ins_info: InstanceCreateInfo = InstanceCreateInfo {
        application_name: Some("me.mhsandayo.vulkano_test".to_string()),
        application_version: Version::major_minor(0, 1),
        enabled_extensions: extensions,
        enabled_layers,
        ..Default::default()
    };

    // アプリ毎にインスタンス作成の必要あり
    let instance = makeins(ins_info);

    //エラーなどデバッグに役立つ情報を出力してくれる
    //別にDebugUtilsMessenger自体は特に使ってないがmust useらしい?。
    //Instanceを渡してあげているのでそこから勝手に独立して動く。
    let _callback = unsafe {
        DebugUtilsMessenger::new(
            instance.clone(),
            DebugUtilsMessengerCreateInfo {
                message_severity: DebugUtilsMessageSeverity::all(),
                message_type: DebugUtilsMessageType::all(),
                ..DebugUtilsMessengerCreateInfo::user_callback(Arc::new(|msg| {
                    let severity = if msg.severity.error {
                        "ERROR!"
                    } else if msg.severity.warning {
                        "warning"
                    } else if msg.severity.information {
                        "information"
                    } else if msg.severity.verbose {
                        "verbose"
                    } else {
                        unreachable!()
                    };
                    let msgtype = if msg.ty.general {
                        "GENERAL"
                    } else if msg.ty.validation {
                        "VALIDATION"
                    } else if msg.ty.performance {
                        "PERFORMANCE"
                    } else {
                        unreachable!()
                    };
                    println!(
                        "{}: [{}] {} - {}",
                        msg.layer_prefix.unwrap_or("unknown"),
                        msgtype,
                        severity,
                        msg.description
                    )
                }))
            },
        )
        .unwrap()
    };

    // 有効にしたい拡張機能をDeviceExtensions構造体に列挙
    let extensions = DeviceExtensions {
        // スワップチェーンを作るためのメソッドに必要らしい
        khr_swapchain: true,
        // UGLY: Supportedなものを全て有効にしたかったがエラーはいたので必要最小限。
        // ..*PhysicalDevice::supported_extensions(&device)
        ..DeviceExtensions::none()
    };

    //物理デバイス（GPU）取得。enumerateは利用可能な物理デバイスのイテレータを返す。
    //next()メソッドは一番最初のItemを取り出すので一番最初の利用可能なデバイスがphysicalに入る
    let (device, mut queues) = PhysicalDevice::enumerate(&instance)
        .filter(|device| {
            device
                // 対応している拡張機能全て取得
                .supported_extensions()
                // たぶん?extensionsに対応/上位互換の拡張がある場合true。trueになったものだけ残して次のfilter_mapへ。
                .is_superset_of(&extensions)
        })
        .filter_map(|device| {
            device
                .queue_families()
                .filter(|queue_family| queue_family.supports_graphics())
                .map(|queue_family| (device, queue_family))
                .next()
        })
        // 論理デバイスの作成。
        // ここの説明がわかりやすい。要するに物理デバイスは一つとかしか無いわけだから独り占めできない、なら一つのプロセスのために仮想的なデバイスを作ってくれるよう頼んで、それをそのプロセスで独り占めできるようにするってこと？。
        // https://foolslab.net/do/vulkan/2-2
        .find_map(|(device, queue_family)| {
            Device::new(
                device,
                DeviceCreateInfo {
                    enabled_extensions: extensions,
                    // enabled_extensions: *PhysicalDevice::supported_extensions(&device),
                    enabled_features: Features::none(),
                    queue_create_infos: vec![QueueCreateInfo::family(queue_family)],
                    ..Default::default()
                },
            )
            .ok()
        })
        .expect("Could not find any GPU");

    // 描画するためにはWindowを作った上でSurfaceをビルドする必要がある
    let eventloop = EventLoop::new();

    //Surfaceを作るために設定する。VulkanでレンダリングしたものをWindowに描画するために必要。
    let surface = WindowBuilder::new()
        //タイトル
        .with_title(String::from("Triangle"))
        //ただのWindowではなくSurfaceを作りたいのでEventloopとinstanceを渡す。
        .build_vk_surface(&eventloop.deref(), instance)
        .unwrap();

    let (swapchain, swapchain_image) = Swapchain::new(
        device.clone(),
        surface.clone(),
        SwapchainCreateInfo {
            image_usage: ImageUsage::color_attachment(),
            ..Default::default()
        },
    )
    .unwrap();

    let graphics_queue = queues
        .find(|queue| queue.family().supports_graphics())
        .unwrap();

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, Zeroable, Pod)]
    struct Vertex {
        position: [f32; 2],
    }
    impl_vertex!(Vertex, position);

    let vertices = [
        Vertex {
            position: [-0.5, -0.25],
        },
        Vertex {
            position: [0.0, 0.5],
        },
        Vertex {
            position: [0.25, -0.1],
        },
    ];

    let vertex_buffer =
        CpuAccessibleBuffer::from_iter(device.clone(), BufferUsage::all(), false, vertices)
            .unwrap();

    // ここからシェーダ。まるまるコピー(今回三角形書くのはあくまでテスト)
    // https://github.com/vulkano-rs/vulkano/blob/master/examples/src/bin/triangle.rs
    mod vs {
        vulkano_shaders::shader! {
            ty: "vertex",
            src: "
            #version 450
            layout(location = 0) in vec2 position;
            void main() {
              gl_Position = vec4(position, 0.0, 1.0);
            }
          "
        }
    }

    mod fs {
        vulkano_shaders::shader! {
            ty: "fragment",
            src: "
            #version 450
            layout(location = 0) out vec4 f_color;
            void main() {
              f_color = vec4(1.0, 0.0, 0.0, 1.0);
            }
          "
        }
    }

    let vs = vs::load(device.clone()).unwrap();
    let fs = fs::load(device.clone()).unwrap();

    let render_pass = single_pass_renderpass!(device.clone(),
    attachments: {

      color: {
          load: Clear,
          store: Store,
          format: swapchain.image_format(),
          samples: 1,
      }

    },
    pass: {
        color: [color],
        depth_stencil: {}
    })
    .unwrap();

    let mut viewport = Viewport {
        origin: [0.0, 0.0],
        depth_range: 0.0..1.0,
        dimensions: [0.0, 0.0],
    };

    let subpass = { Subpass::from(render_pass.clone(), 0).unwrap() };

    let graphics_pipeline = GraphicsPipeline::start()
        .vertex_input_state(BuffersDefinition::new().vertex::<Vertex>())
        .vertex_shader(vs.entry_point("main").unwrap(), ())
        .fragment_shader(fs.entry_point("main").unwrap(), ())
        .viewport_state(ViewportState::viewport_dynamic_scissor_irrelevant())
        .rasterization_state(RasterizationState::default())
        .render_pass(subpass)
        .build(device.clone())
        .expect("Oh, no. Couldn't create graphics pipeline.");

    let dimensions = swapchain_image[0].dimensions().width_height();
    viewport.dimensions = [dimensions[0] as f32, dimensions[1] as f32];
    let view = ImageView::new_default(swapchain_image.into_iter().next().unwrap()).unwrap();

    let framebuffer = Framebuffer::new(
        render_pass.clone(),
        FramebufferCreateInfo {
            attachments: vec![view],
            ..Default::default()
        },
    )
    .unwrap();

    let mut builder = AutoCommandBufferBuilder::primary(
        device.clone(),
        device.clone().active_queue_families().next().unwrap(),
        CommandBufferUsage::OneTimeSubmit,
    )
    .unwrap();

    let clear_values = vec![Some(ClearValue::Float([0.0, 0.0, 1.0, 1.0]))];
    builder
        .begin_render_pass(
            RenderPassBeginInfo {
                clear_values,
                ..RenderPassBeginInfo::framebuffer(framebuffer)
            },
            SubpassContents::Inline,
        )
        .unwrap()
        .set_viewport(0, [viewport.clone()])
        .bind_pipeline_graphics(graphics_pipeline.clone())
        .bind_vertex_buffers(0, vertex_buffer.clone())
        .draw(vertex_buffer.len() as u32, 1, 0, 0)
        .unwrap()
        .end_render_pass()
        .unwrap();

    let command_buffer = Arc::new(builder.build().unwrap());

    eventloop.run(move |event, _, ctrlflow| {
        // ここはイベントが発生したときに呼ばれる実行されるクロージャ
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                window_id,
            } => {
                if window_id == surface.clone().window().id() {
                    *ctrlflow = ControlFlow::Exit;
                }
            }

            Event::RedrawEventsCleared => {
                surface.window().request_redraw();
            }

            Event::RedrawRequested(_) => {
                if let Ok((image_index, _, acquire_future)) =
                    acquire_next_image(swapchain.clone(), None)
                {
                    if let Ok(future) =
                        acquire_future.then_execute(graphics_queue.clone(), command_buffer.clone())
                    {
                        let _ = future
                            .then_swapchain_present(
                                graphics_queue.clone(),
                                swapchain.clone(),
                                image_index,
                            )
                            .then_signal_fence_and_flush()
                            .and_then(|future| future.wait(None));
                    }
                }
            }
            _ => {
                *ctrlflow = ControlFlow::Wait;
                ()
            }
        }
    })
}

// TODO: InstanceCreateInfoまで自動生成したい
fn makeins(ins_info: InstanceCreateInfo) -> Arc<Instance> {
    return Instance::new(ins_info).expect("Could not create an Instance.");
}
