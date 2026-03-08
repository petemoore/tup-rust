// tup-types: Core types and data structures for the tup build system
//
// Copyright (C) 2009-2024  Mike Shal <marfey@gmail.com>
// Rust port Copyright (C) 2026  Pete Moore
//
// This program is free software; you can redistribute it and/or modify
// it under the terms of the GNU General Public License version 2 as
// published by the Free Software Foundation.

mod access_event;
mod constants;
mod error;
mod flags;
mod link_type;
mod node_type;
mod tupid;

pub use access_event::AccessType;
pub use constants::*;
pub use error::TupError;
pub use flags::{FlagSet, TupFlags};
pub use link_type::LinkType;
pub use node_type::NodeType;
pub use tupid::TupId;
