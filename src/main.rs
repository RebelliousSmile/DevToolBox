//! WinFXStart - Windows 11 Command Launcher
//! 
//! Application Rust avec WinUI 3 pour lancer des commandes CLI facilement.

use std::env;
use tao::event_loop::{EventLoop, ControlFlow};
use tao::window::{WindowBuilder, WindowLevel};

fn main() {
    // Initialiser la logger
    env_logger::init();

    println!("WinFXStart v0.1.0");
    println!("Lancement au démarrage activé...");

    // Configuration de l'event loop
    let event_loop = EventLoop::new().unwrap();
    
    // Créer la fenêtre principale
    let window = WindowBuilder::new()
        .with_title("WinFXStart - Command Launcher")
        .with_inner_size(tao::dpi::LogicalSize::new(800, 600))
        .with_min_inner_size(tao::dpi::LogicalSize::new(400, 300))
        .with_window_level(WindowLevel::Overlay)
        .build(&event_loop)
        .unwrap();

    // Démarrer l'application
    tao::application::run(event_loop, window);
}
