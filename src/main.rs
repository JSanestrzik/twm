use std::{time::Duration, os::fd::AsRawFd};
use std::sync::Arc;

use anyhow::{Result, Context, Ok};
use smithay::backend::input::{AbsolutePositionEvent, PointerButtonEvent, ButtonState, PointerAxisEvent};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::desktop::WindowSurfaceType;
use smithay::input::pointer::{MotionEvent, ButtonEvent};
use smithay::wayland::data_device::{DataDeviceHandler, ServerDndGrabHandler, ClientDndGrabHandler};
use smithay::wayland::seat::WaylandFocus;
use smithay::{
    desktop::{
        Space,
        Window
    },
    output::{
        Output,
        Mode,
        PhysicalProperties,
        Subpixel
    },
    backend::{
        input::{InputEvent, KeyboardKeyEvent, Event, Axis},
        renderer::gles::GlesRenderer,
        renderer::utils::on_commit_buffer_handler,
        winit::{self, WinitEvent, WinitError},
    },
    reexports::{
        calloop::{ PostAction, Interest, EventLoop, timer::{Timer, TimeoutAction}, LoopSignal, generic::Generic},
        wayland_server::{
            protocol::{
                wl_data_source::WlDataSource,
                wl_data_device_manager::DndAction,
                wl_surface::WlSurface,
                wl_buffer::WlBuffer,
                wl_seat::WlSeat,
                wl_output::WlOutput,
            },
            Client,
            Display, backend::ClientData
        },
        wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
        wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1,
    }, 
    wayland::{
        data_device::DataDeviceState,    
        socket::ListeningSocketSource,
        compositor::{
            CompositorState, 
            CompositorHandler, 
            CompositorClientState,
            get_parent,
            is_sync_subsurface,
            with_states,
        }, 
        shell::xdg::{
            XdgShellState, 
            XdgShellHandler,
            ShellClient,
            PopupSurface,
            ToplevelSurface,
            Configure,
            PositionerState,
        }, 
        shm::{
            ShmState, 
            ShmHandler
        }, 
        buffer::BufferHandler,
    },
    utils::{
        Rectangle, 
        Serial,
        Logical,
        Point,
        SERIAL_COUNTER,
    },
    delegate_compositor, delegate_shm, delegate_xdg_shell, delegate_seat, delegate_output, delegate_data_device,
    input::{
        keyboard::{FilterResult},
        pointer::AxisFrame,
        SeatState, Seat, SeatHandler},
};


#[derive(Default)]
struct TwmClientState {
    compositor_state: CompositorClientState
}

impl ClientData for TwmClientState {
    fn initialized(&self, client_id: smithay::reexports::wayland_server::backend::ClientId) {
        println!("Initialized client wih id: {:?}", client_id);
    }

    fn disconnected(&self, client_id: smithay::reexports::wayland_server::backend::ClientId, 
                    reason: smithay::reexports::wayland_server::backend::DisconnectReason) {
        println!("Client with id: {:?} disconnected with reason: {:?}", client_id, reason);
    }

    fn debug(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(format!("TwmClient").as_str())
    }

}


struct TwmState {
    start_time: std::time::Instant,
    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    shm_state: ShmState,
    seat_state: SeatState<Self>,
    data_device_state: DataDeviceState,

    space: Space<Window>,

    ev_signal: LoopSignal,

    seat: Seat<Self>,
}


impl TwmState {
    fn new(event_loop: &mut EventLoop<TwmLoopData>, 
           display: &mut Display<Self>) -> Result<Self> {
        let display_handle = display.handle();
        
        let compositor_state = CompositorState::new::<TwmState>(&display_handle);
        let shm_state = ShmState::new::<TwmState>(&display_handle, vec![]);
        let xdg_shell_state = XdgShellState::new::<TwmState>(&display_handle);
       
        let mut seat_state = SeatState::new();
        let seat = seat_state.new_wl_seat(&display_handle, "winit");
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);

        let ev_signal = event_loop.get_signal();

        Ok(Self {
            start_time: std::time::Instant::now(),
            compositor_state,
            xdg_shell_state,
            shm_state,
            seat_state,
            data_device_state,
            space: Space::default(),
            ev_signal,
            seat,
        })
    }

    pub fn surface_under(&self, position: Point<f64, Logical>) -> Option<(WlSurface, Point<i32, Logical>)> {
        self.space.element_under(position).and_then(|(window, location)| {
            window
                .surface_under(position - location.to_f64(), WindowSurfaceType::ALL)
                .map(|(s,p)| (s, p + location))
        })
    }
}

impl SeatHandler for TwmState {
    type PointerFocus = WlSurface;
    type KeyboardFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: smithay::input::pointer::CursorImageStatus) {
        //println!("Cursor image");
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&Self::KeyboardFocus>) {
       println!("Focus changed"); 
    }
}

impl CompositorHandler for TwmState {
    fn commit(&mut self, surface: &WlSurface) {
        println!("Commit");
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }

            if let Some(window) = self.space
                .elements()
                .find(|w| w.toplevel().wl_surface() == &root) {
                window.on_commit();
            }
        }
    }

    fn new_surface(&mut self, surface: &WlSurface) {  
        println!("new surface");
    }

    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<TwmClientState>().unwrap().compositor_state
    }

    fn destroyed(&mut self, _surface: &WlSurface) {
        println!("Destroyed surfact");
    }
}

impl ShmHandler for TwmState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl XdgShellHandler for TwmState {
    fn new_client(&mut self, client: ShellClient) {
        println!("new client: {:?}", client);
    }

    fn new_popup(&mut self, 
                 surface: PopupSurface,
                 positioner: PositionerState) {
        println!("New popup");
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
       println!("New top level"); 
        let window = Window::new(surface);
        self.space.map_element(window, (0, 0), false);
        //let pointer = self.seat.get_pointer().unwrap();  
        //let output = self.space
        //    .output_under(pointer.current_location());

        //let geometry = output.
    }

    fn client_pong(&mut self, client: ShellClient) {
        println!("clieng pont");
    }

    fn grab(&mut self, 
            surface: PopupSurface, 
            seat: WlSeat, 
            serial: Serial) {
        println!("grap");
    }

    fn ack_configure(&mut self, surface: WlSurface, configure: Configure) {
       println!("Ack configure"); 
    }

    fn move_request(&mut self, 
                    surface: ToplevelSurface, 
                    seat: WlSeat, 
                    serial: Serial) {
       println!("move request"); 

    }

    fn resize_request(
            &mut self,
            surface: ToplevelSurface,
            seat: WlSeat,
            serial: Serial,
            edges: ResizeEdge,
        ) {
        println!("Resize request");
    }

    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn popup_destroyed(&mut self, surface: PopupSurface) {
        println!("Popup destroyed");
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
       println!("maximize request"); 

    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
       println!("Minimize request"); 
    }

    fn show_window_menu(
            &mut self,
            surface: ToplevelSurface,
            seat: WlSeat,
            serial: Serial,
            location: Point<i32, Logical>,
        ) {
       println!("Shod window menu"); 

    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
       println!("Unmaximize request"); 
    }

    fn fullscreen_request(&mut self, 
                          surface: ToplevelSurface, 
                          output: Option<WlOutput>) {
        println!("Fullscreen request");
    }

    fn reposition_request(&mut self, 
                          surface: PopupSurface, 
                          positioner: PositionerState, 
                          token: u32) {
       println!("Reposition request"); 

    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
       println!("Toplevel destroyed"); 
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
       println!("Unfullscreen request"); 
    }
}

impl BufferHandler for TwmState {
    fn buffer_destroyed(&mut self, buffer: &WlBuffer) {
       println!("Buffer destroyed"); 
    }
}

impl DataDeviceHandler for TwmState {
    type SelectionUserData = ();
    fn action_choice(&mut self, available: DndAction, preferred: DndAction) -> DndAction {
        println!("Action choice");
        preferred
    }

    fn new_selection(&mut self, source: Option<WlDataSource>, seat: Seat<Self>) {
        println!("new selectio");
    }

    fn send_selection(
            &mut self,
            mime_type: String,
            fd: std::os::fd::OwnedFd,
            seat: Seat<Self>,
            user_data: &Self::SelectionUserData,
        ) {
        println!("Send selection");
    }

    fn data_device_state(&self) -> &DataDeviceState {
       &self.data_device_state
    }
}

impl ClientDndGrabHandler for TwmState {
    fn started(&mut self, source: Option<WlDataSource>, icon: Option<WlSurface>, seat: Seat<Self>) {
        println!("Client dnd grab started");
    }

    fn dropped(&mut self, seat: Seat<Self>) {
        println!("Client dhd grab dropped");
    }
}

impl ServerDndGrabHandler for TwmState {
    fn dropped(&mut self, seat: Seat<Self>) {
       println!("Server dnd grab deopped"); 
    }

    fn cancelled(&mut self, seat: Seat<Self>) {
        println!("Server dnd grab cancelled");
    }

    fn finished(&mut self, seat: Seat<Self>) {
        println!("Server dnd grab finished");
    }

    fn action(&mut self, action: DndAction, seat: Seat<Self>) {
        println!("Served dnd grab action");
    }

    fn accept(&mut self, mime_type: Option<String>, seat: Seat<Self>) {
        println!("Server dnd grab accept");
    }

    fn send(&mut self, mime_type: String, fd: std::os::fd::OwnedFd, seat: Seat<Self>) {
        println!("Server dnd grab send");
    }
}

struct TwmLoopData {
    display: Display<TwmState>,
    state: TwmState,
}

fn main() -> Result<()>  {

    let current_display = std::env::var("WAYLAND_DISPLAY");
    println!("TWM Starting");

    let mut display: Display<TwmState> = Display::new().context("Failed to get wayland display")?;
    let mut event_loop: EventLoop<TwmLoopData> = EventLoop::try_new()
        .context("Couldn't create event loop")?;

    let mut state = TwmState::new(&mut event_loop, &mut display)
        .context("Failed to initialize compositor state")?;

    let (mut gfx_backend, mut winit_el) = winit::init::< GlesRenderer>().expect("Failed to Initialize a graphics and input backend");
    let keyboard = state.seat.add_keyboard(Default::default(), 200, 200).context("Failed to init keyboard")?;
    state.seat.add_pointer();


    event_loop
        .handle()
        .insert_source(
            Generic::new(
                display.backend().poll_fd().as_raw_fd(), 
                Interest::READ,
                smithay::reexports::calloop::Mode::Level), 
            |_, _, data| {
                data.display.dispatch_clients(&mut data.state).expect("Dispatch state to clients");
                std::io::Result::Ok(PostAction::Continue)
            })
    .context("Failed to insert display fd source into event loop")?;

    

    println!("State initialized!");

    let socket = ListeningSocketSource::new_auto().context("Failed to open socket")?;
    let socket_name = socket.socket_name().to_os_string();
    std::env::set_var("WAYLAND_DISPLAY", socket_name.clone());
    println!("Updated wayland display to: {:?}", socket_name);

    let output = Output::new(
        "winit".to_string(),
        PhysicalProperties { 
            size: (0, 0).into(), // initial size
            subpixel: Subpixel::Unknown, // sub bixel setting
            make: "Twm".into(), // monitor manufacturer 
            model: "Winit".into() // monitor model
        }
    );
    
    let mode = Mode {
        size: gfx_backend.window_size().physical_size,
        refresh: 60_000,
    };

    println!("window size {:?}", gfx_backend.window_size());

    let _global = output.create_global::<TwmState>(&display.handle());
    output.change_current_state(Some(mode), Some(smithay::utils::Transform::Flipped180), None, Some((0, 0).into()));
    output.set_preferred(mode);

    state.space.map_output(&output, (0, 0));

    let mut damage_tracker = OutputDamageTracker::from_output(&output);

    
    let timert = Timer::immediate();
    event_loop.handle().insert_source(timert, move |_, _, data| {

       let res = winit_el.dispatch_new_events(|event| match event {
           WinitEvent::Input(input_event) => match input_event {
               InputEvent::Keyboard { event } => {
                   let serial = SERIAL_COUNTER.next_serial();
                   let time = Event::time_msec(&event);

                   keyboard.input::<(), _>(
                       &mut data.state, // composer state
                       event.key_code(), // keyboard key code
                       event.state(), // keoboard event state
                       serial,
                       time,
                       |_, _, _| { // Event filter block
                           println!("pressed: {}", event.key_code());
                           FilterResult::Forward // Filter result we forward everything
                       }
                    );
               },
               InputEvent::PointerMotionAbsolute { event } => {
                    let output = data.state.space.outputs().next().expect("Output avaiable");
                    let geometry_output = data.state.space.output_geometry(output).expect("Geometry output available");
                    let position = event.position_transformed(geometry_output.size) + geometry_output.loc.to_f64();
                    let serial = SERIAL_COUNTER.next_serial();
                    let pointer = data.state.seat.get_pointer().expect("Pointer available");
                    let surface_under_pointer = data.state.surface_under(position);

                    pointer.motion(&mut data.state, surface_under_pointer, &MotionEvent {
                        location: position,
                        serial,
                        time: event.time_msec()
                    });
                },
                InputEvent::PointerButton { event } => {
                    let pointer = data.state.seat.get_pointer().expect("Pointer available");
                    let keyboard = data.state.seat.get_keyboard().expect("Keyboard available");
                    let serial = SERIAL_COUNTER.next_serial();
                    let button = event.button_code();
                    let buton_state = event.state();

                    if ButtonState::Pressed == buton_state && !pointer.is_grabbed() {
                        if let Some((window, _location)) = data.state
                                .space
                                .element_under(pointer.current_location())
                                .map(|(w, l)| (w.clone(), l)) {
                            print!("clicked on window");
                            data.state.space.raise_element(&window, true);
                            keyboard.set_focus(&mut data.state, Some(window.toplevel().wl_surface().clone()), serial);
                            data.state.space.elements().for_each(|window| {
                                window.toplevel().send_pending_configure();
                            });
                            println!("Update focus");
                        } else {
                            data.state.space.elements().for_each(|window| {
                                window.set_activated(false);
                                window.toplevel().send_pending_configure();
                            });
                            keyboard.set_focus(&mut data.state, Option::<WlSurface>::None, serial);
                            println!("Reset focus");
                        }

                        pointer.button(
                            &mut data.state,
                            &ButtonEvent {
                                button,
                                state: buton_state,
                                serial,
                                time: event.time_msec()
                            }
                        );
                    }
                },
                InputEvent::PointerAxis { event } => {
                    let source = event.source();

                    let horizontal_amount = event.amount(Axis::Horizontal)
                        .unwrap_or_else(|| event.amount_discrete(Axis::Horizontal).unwrap_or(0.0) * 3.0);
                    let vertical_amount = event.amount(Axis::Vertical)
                        .unwrap_or_else(|| event.amount_discrete(Axis::Vertical).unwrap_or(0.0) * 3.0);
                    let horizontal_amount_dis = event.amount_discrete(Axis::Horizontal);
                    let vertical_amount_dis = event.amount_discrete(Axis::Vertical);

                    let mut frame = AxisFrame::new(event.time_msec()).source(source);

                    if horizontal_amount != 0.0 {
                        frame = frame.value(Axis::Horizontal, horizontal_amount);
                        if let Some(value) = horizontal_amount_dis {
                            frame = frame.discrete(Axis::Horizontal, value as i32);
                        }
                    }
                    if vertical_amount != 0.0 {
                        frame = frame.value(Axis::Vertical, vertical_amount);
                        if let Some(value) = vertical_amount_dis {
                            frame = frame.discrete(Axis::Vertical, value as i32);
                        }
                    }


                    data.state.seat.get_pointer().expect("Pointer available").axis(&mut data.state, frame);
                },
               _ => {}
           },
           _ => {}
       });

       if let Err(WinitError::WindowClosed) = res { // if this happens our composer got closed
           data.state.ev_signal.stop(); // Since the composer stopped we stop the whole event loop
           return TimeoutAction::Drop;
       } else {
           res.expect("Failed to dispatch new events on input event loop"); // Somethng else went
                                                                            // wrong 
       }

       gfx_backend.bind().expect("Failed to bind gfx context"); // Bind the graphics backend

       let size = gfx_backend.window_size().physical_size; // Physical size of the main display
                                                           // window
       let damage = Rectangle::from_loc_and_size((0,0), size); // Damage rectangle covering the
                                                               // whole available screen
      

       smithay::desktop::space::render_output::<_, WaylandSurfaceRenderElement<GlesRenderer>, _, _> (
           &output, 
           gfx_backend.renderer(), 
           1.0, 
           0, 
           [&data.state.space],
           &[], 
           &mut damage_tracker, 
           [0.1, 0.1, 0.1, 1.0])
           .expect("Failed to render output");

       gfx_backend.submit(Some(&[damage])).expect("Failed to submit damage on gfx backend");


       data.state.space.elements().for_each(|window| {
           window
               .send_frame(
                   &output, 
                   data.state.start_time.elapsed(), 
                   Some(Duration::ZERO),
                   |_, _| {
                       Some(output.clone())
                   });
       });

       data.state.space.refresh();
       data.display.flush_clients().expect("Flush clients correctly");



       TimeoutAction::ToDuration(Duration::from_millis(16))
    }).expect("Failed to insert new sourc to event loop");

    event_loop.handle().insert_source(socket, move |client_stream, _, data| {
        data.display
            .handle()
            .insert_client(client_stream, Arc::new(TwmClientState::default()))
            .expect("Failed to inset new client");
    }).context("Failed to insert wayland socket source")?;


    std::process::Command::new("alacritty").spawn().context("Failed to spawn process")?;
    std::process::Command::new("alacritty").spawn().context("Failed to spawn process")?;
    
    let mut loop_data = TwmLoopData {
        display,
        state,
    };

    let _ = event_loop.run(None, &mut loop_data, move |_| {}).context("Failed to start event loop")?;    

    println!("TWM finishing working ");

    if let std::result::Result::Ok(socket_name) = current_display {
        std::env::set_var("WAYLAND_DISPLAY", socket_name.clone());
        println!("Reverted wayland display to: {:?}", socket_name.clone());
    }
    Ok(())
}

delegate_shm!(TwmState);
delegate_compositor!(TwmState);
delegate_xdg_shell!(TwmState);
delegate_seat!(TwmState);
delegate_output!(TwmState);
delegate_data_device!(TwmState);
