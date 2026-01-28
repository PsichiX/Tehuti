use std::{
    sync::Arc,
    thread::{JoinHandle, sleep},
    time::Duration,
};
use tehuti::{meeting::MeetingInterface, peer::PeerFactory};
use tehuti_mock::{MockMachine, MockNetwork, MockNetworkPortConfig};

#[derive(Default)]
pub struct Example {
    network: MockNetwork,
    machines: Vec<JoinHandle<()>>,
}

impl Example {
    pub fn machine(
        mut self,
        id: impl ToString,
        config: MockNetworkPortConfig,
        factory: Arc<PeerFactory>,
        delay: Duration,
        f: impl FnOnce(MeetingInterface) + Send + 'static,
    ) -> Self {
        let port = self.network.open_port(id, config).unwrap();
        self.machines
            .push(MockMachine::default().run(move |machine| {
                sleep(delay);
                let meeting = machine.start_meeting(factory, port).unwrap();
                f(meeting);
            }));
        self
    }

    pub fn join(self) {
        for handle in self.machines {
            let _ = handle.join();
        }
    }
}
