// Copyright 2020 Twitter, Inc.
// Licensed under the Apache License, Version 2.0
// http://www.apache.org/licenses/LICENSE-2.0

use serde::{Deserialize, Serialize};

// constants to define default values
const TCP_BACKLOG: usize = 128;
const TCP_POOLSIZE: usize = 0;
const TCP_PRIORITY: u32 = 0;

// helper functions
fn backlog() -> usize {
    TCP_BACKLOG
}

fn poolsize() -> usize {
    TCP_POOLSIZE
}

fn priority() -> u32 {
    TCP_PRIORITY
}

// definitions
#[derive(Serialize, Deserialize, Debug)]
pub struct Tcp {
    #[serde(default = "backlog")]
    backlog: usize,
    #[serde(default = "poolsize")]
    poolsize: usize,
    #[serde(default = "priority")]
    priority: u32,
}

// implementation
impl Tcp {
    pub fn backlog(&self) -> usize {
        self.backlog
    }

    pub fn poolsize(&self) -> usize {
        self.poolsize
    }

    pub fn priority(&self) -> u32 {
        self.priority
    }
}

// trait implementations
impl Default for Tcp {
    fn default() -> Self {
        Self {
            backlog: backlog(),
            poolsize: poolsize(),
            priority: priority(),
        }
    }
}

// trait definitions
pub trait TcpConfig {
    fn tcp(&self) -> &Tcp;
}
