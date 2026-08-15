//! Drive path (P3 opens HTTP endpoints; the command vocabulary and adapter
//! plumbing live here from day one so the read path never reaches into
//! adapters).
//!
//! P1 scope: `DriveCommand` is defined in `crate::adapters` and implemented
//! by the herdr adapter (`prompt`, `interrupt`, `read_tail`); the remaining
//! commands are explicitly `NotImplemented` until P3 claim-based approvals.

pub mod control {
    //! Future home of the per-agent drive controller (approval queue, claim
    //! checks, device signatures). P1: empty by design — do not delete.

    /// Marker for the drive controller seam (P3).
    #[derive(Debug)]
    pub struct DriveController;
}
