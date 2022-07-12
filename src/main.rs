#![allow(irrefutable_let_patterns)]
use slog::Drain;

use std::{ffi::OsString, sync::Arc};

use slog::Logger;
use smithay::{
    desktop::{Space, WindowSurfaceType},
    reexports::{
        calloop::{
            generic::Generic, 
            EventLoop, 
            Interest, 
            LoopSignal, 
            Mode, 
            PostAction,
            timer::{TimeoutAction, Timer},
        },
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{
                wl_surface::WlSurface,
                wl_output,
            },
            Display,
        },
    },
    backend::{
        renderer::gles2::Gles2Renderer,
        winit::{self, WinitError, WinitEvent, WinitEventLoop, WinitGraphicsBackend},
    },
    desktop::space::SurfaceTree,
    utils::{Logical, Point, Rectangle},
    wayland::{
        compositor::CompositorState,
        data_device::DataDeviceState,
        output::OutputManagerState,
        seat::{Seat, SeatState},
        shell::xdg::XdgShellState,
        shm::ShmState,
        socket::ListeningSocketSource,
    },
    wayland::output::{Mode as OutputMode, Output, PhysicalProperties},
};

use std::time::Duration;

mod handlers;
mod grabs;

pub struct State {
    pub space: Space,
    pub loop_signal: LoopSignal,
    pub log: slog::Logger,

    pub socket_name: OsString,
    pub start_time: std::time::Instant,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<State>,
    pub data_device_state: DataDeviceState, 
}

pub struct CalloopData {
    state: State,
    display: Display<State>,
}

impl State {
    pub fn new(event_loop: &mut EventLoop<CalloopData>, display: &mut Display<Self>, log: Logger) -> Self {
        let dh = display.handle();
        let start_time = std::time::Instant::now();

        let compositor_state = CompositorState::new::<Self, _>(&dh, log.clone());
        let xdg_shell_state = XdgShellState::new::<Self, _>(&dh, log.clone());
        let shm_state = ShmState::new::<Self, _>(&dh, vec![], log.clone());
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self, _>(&dh, log.clone());

        let space = Space::new(log.clone());
        let loop_signal = event_loop.get_signal();

        let socket_name = Self::init_wayland_listener(display, event_loop, log.clone());

        Self {
            space,
            loop_signal,
            log,

            socket_name,
            start_time,

            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
        }
    }

     fn init_wayland_listener(
        display: &mut Display<State>,
        event_loop: &mut EventLoop<CalloopData>,
        log: slog::Logger,
    ) -> OsString {
        // Creates a new listening socket, automatically choosing the next available `wayland` socket name.
        let listening_socket = ListeningSocketSource::new_auto(log).unwrap();

        // Get the name of the listening socket.
        // Clients will connect to this socket.
        let socket_name = listening_socket.socket_name().to_os_string();

        let handle = event_loop.handle();

        event_loop
            .handle()
            .insert_source(listening_socket, move |client_stream, _, state| {
                // Inside the callback, you should insert the client into the display.
                //
                // You may also associate some data with the client when inserting the client.
                state
                    .display
                    .handle()
                    .insert_client(client_stream, Arc::new(ClientState))
                    .unwrap();
            })
            .expect("Failed to init the wayland event source.");

        // You also need to add the display itself to the event loop, so that client events will be processed by wayland-server.
        handle
            .insert_source(
                Generic::new(display.backend().poll_fd(), Interest::READ, Mode::Level),
                |_, _, state| {
                    state.display.dispatch_clients(&mut state.state).unwrap();
                    Ok(PostAction::Continue)
                },
            )
            .unwrap();

        socket_name
    }
}

pub struct ClientState;
impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

pub fn winit_backend(
    event_loop: &mut EventLoop<CalloopData>,
    data: &mut CalloopData,
    log: Logger,
) -> Result<(), Box<dyn std::error::Error>> {
    let display = &mut data.display;
    let state = &mut data.state;

    let (mut backend, mut winit) = winit::init(log.clone())?;

    let mode = OutputMode {
        size: backend.window_size().physical_size,
        refresh: 60_000,
    };

    let output = Output::new::<_>(
        "winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: wl_output::Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Winit".into(),
        },
        log.clone(),
    );

    let _global = output.create_global::<State>(&display.handle());

    output.change_current_state(
        Some(mode),
        Some(wl_output::Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );

    output.set_preferred(mode);

    state.space.map_output(&output, (0, 0));

    std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);

    let mut full_redraw = 0u8;

    let timer = Timer::immediate();
    event_loop.handle().insert_source(timer, move |_, _, data| {
        winit_dispatch(&mut backend, &mut winit, data, &output, &mut full_redraw).unwrap();
        TimeoutAction::ToDuration(Duration::from_millis(16))
    })?;
    
    Ok(())
}

pub fn winit_dispatch(
    backend: &mut WinitGraphicsBackend,
    winit: &mut WinitEventLoop,
    data: &mut CalloopData,
    output: &Output,
    full_redraw: &mut u8,
) -> Result<(), Box<dyn std::error::Error>> {
    let display = &mut data.display;
    let state = &mut data.state;

    let res = winit.dispatch_new_events(|event| match event {
        WinitEvent::Resized { size, .. } => {
            output.change_current_state(
                Some(OutputMode {
                    size,
                    refresh: 60_000,
                }),
                None,
                None,
                None,
            );
        }
        WinitEvent::Input(event) => {
            println!("Input event: {:?}", event);
        },
        _ => (),
    });

    if let Err(WinitError::WindowClosed) = res {
        // Stop the loop
        state.loop_signal.stop();

        return Ok(());
    } else {
        res?;
    }

    *full_redraw = full_redraw.saturating_sub(1);

    let size = backend.window_size().physical_size;
    let damage = Rectangle::from_loc_and_size((0, 0), size);

    backend.bind().ok().and_then(|_| {
        state
            .space
            .render_output::<Gles2Renderer, SurfaceTree>(
                backend.renderer(),
                output,
                0,
                [0.1, 0.1, 0.1, 1.0],
                &[],
            )
            .unwrap()
    });

    backend.submit(Some(&[damage])).unwrap();

    state
        .space
        .send_frames(state.start_time.elapsed().as_millis() as u32);

    state.space.refresh(&display.handle());
    display.flush_clients()?;

    Ok(())
}

fn main() {
    let log = ::slog::Logger::root(::slog_stdlog::StdLog.fuse(), slog::o!());
    slog_stdlog::init().unwrap();

    let mut event_loop: EventLoop<CalloopData> = EventLoop::try_new().unwrap();

    let mut display: Display<State> = Display::new().unwrap();
    let state = State::new(&mut event_loop, &mut display, log.clone());

    let mut data = CalloopData { state, display };

    winit_backend(&mut event_loop, &mut data, log).unwrap();

    std::process::Command::new("alacritty").spawn().ok();

    event_loop.run(None, &mut data, move |_| {
        // Episk is running
    }).unwrap();
}
