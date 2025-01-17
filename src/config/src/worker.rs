// Copyright 2020 Twitter, Inc.
// Licensed under the Apache License, Version 2.0
// http://www.apache.org/licenses/LICENSE-2.0

use serde::{Deserialize, Serialize};

// constants to define default values
const WORKER_TIMEOUT: usize = 100;
const WORKER_NEVENT: usize = 1024;
const WORKER_THREADS: usize = 1;

// default worker balance strategy
const WORKER_BALANCE: Balance = Balance::Random;

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum Balance {
    Random,
    Queues,
}

// helper functions
fn timeout() -> usize {
    WORKER_TIMEOUT
}

fn nevent() -> usize {
    WORKER_NEVENT
}

fn threads() -> usize {
    WORKER_THREADS
}

fn balance() -> Balance {
    WORKER_BALANCE
}

// definitions
#[derive(Serialize, Deserialize, Debug)]
pub struct Worker {
    #[serde(default = "timeout")]
    timeout: usize,
    #[serde(default = "nevent")]
    nevent: usize,
    #[serde(default = "threads")]
    threads: usize,
    #[serde(default = "balance")]
    balance: Balance,
}

// implementation
impl Worker {
    pub fn timeout(&self) -> usize {
        self.timeout
    }

    pub fn nevent(&self) -> usize {
        self.nevent
    }

    pub fn threads(&self) -> usize {
        self.threads
    }

    pub fn balance(&self) -> Balance {
        self.balance
    }

    pub fn set_threads(&mut self, threads: usize) {
        self.threads = threads
    }
}

// trait implementations
impl Default for Worker {
    fn default() -> Self {
        Self {
            timeout: timeout(),
            nevent: nevent(),
            threads: threads(),
            balance: balance(),
        }
    }
}

pub trait WorkerConfig {
    fn worker(&self) -> &Worker;

    fn worker_mut(&mut self) -> &mut Worker;
}
