mod multi_spike;
mod shell_spike;
mod state;

pub use multi_spike::run as run_multi_spike;
pub use shell_spike::{run as run_shell_spike, run_nvim as run_nvim_spike, run_pi as run_pi_spike};
pub use state::App;
