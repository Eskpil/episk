mod compositor;
mod xdg_shell;

use crate::State;

//
// Wl Seat
//

use smithay::wayland::data_device::{ClientDndGrabHandler, DataDeviceHandler, ServerDndGrabHandler};
use smithay::wayland::seat::{SeatHandler, SeatState};
use smithay::{delegate_data_device, delegate_output, delegate_seat};

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState<State> {
        &mut self.seat_state
    }
}

delegate_seat!(State);

//
// Wl Data Device
//

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &smithay::wayland::data_device::DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {}

delegate_data_device!(State);

//
// Wl Output & Xdg Output
//

delegate_output!(State);
