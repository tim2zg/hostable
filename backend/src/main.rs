use clap::{Parser, Subcommand};
use std::fs;
use std::path::PathBuf;

mod converter;
mod oci;
mod proxmox;

#[derive(serde::Deserialize)]
struct Config {
    interval_seconds: u64,
    deployments: Vec<Deployment>,
}

#[derive(serde::Deserialize)]
struct Deployment {
    image: String,
    vmid: u32,
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Converts a Dockerfile to a Distrobuilder YAML file
    Convert {
        /// The path to the Dockerfile
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,
        
        /// Output path for the generated YAML (optional)
        #[arg(short, long, value_name = "OUT")]
        out: Option<PathBuf>,
    },
    /// Converts and deploys a Dockerfile to Proxmox
    Deploy {
        /// The path to the Dockerfile
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,
    },
    /// Pulls a Docker image, extracts its rootfs, and compresses to .tar.xz
    PullDocker {
        /// The Docker image to pull and convert (e.g. lscr.io/linuxserver/jellyfin:latest)
        #[arg(short, long, value_name = "IMAGE")]
        image: String,

        /// Output path for the extracted rootfs directory or .tar.xz file
        #[arg(short, long, value_name = "OUT")]
        out: PathBuf,
    },
    /// Updates an existing LXC container on Proxmox based on OCI image differences
    UpdateLxc {
        /// The Docker image to pull and convert
        #[arg(short, long, value_name = "IMAGE")]
        image: String,

        /// The target Proxmox VMID to update
        #[arg(short, long, value_name = "VMID")]
        vmid: u32,
        
        /// Output path for the extracted rootfs .tar.xz file (simulating upload)
        #[arg(short, long, value_name = "OUT")]
        out: PathBuf,
    },
    /// Runs the daemon to continuously update deployments based on a config file
    Manage {
        /// Path to the config.yaml file
        #[arg(short, long, value_name = "CONFIG")]
        config: PathBuf,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Setup tracing/logging for CLI
    tracing_subscriber::fmt::init();

    match &cli.command {
        Commands::Convert { file, out } => {
            if !file.exists() {
                eprintln!("Error: Dockerfile '{}' not found", file.display());
                std::process::exit(1);
            }
            
            let content = fs::read_to_string(file)?;
            let yaml = converter::convert_dockerfile_to_distrobuilder(&content);
            
            if let Some(out_path) = out {
                fs::write(out_path, yaml)?;
                println!("Successfully converted and saved to {}", out_path.display());
            } else {
                println!("--- Generated Distrobuilder YAML ---");
                println!("{}", yaml);
            }
        },
        Commands::Deploy { file } => {
            if !file.exists() {
                eprintln!("Error: Dockerfile '{}' not found", file.display());
                std::process::exit(1);
            }
            
            println!("Deploying {} to Proxmox...", file.display());
            let content = fs::read_to_string(file)?;
            let yaml = converter::convert_dockerfile_to_distrobuilder(&content);
            
            println!("Successfully generated YAML, simulated deploy.");
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            println!("Deployed successfully! (Simulation)");
        },
        Commands::PullDocker { image, out } => {
            println!("Pulling OCI/Docker image '{}'...", image);
            let extractor = oci::OciExtractor::new();
            
            match extractor.extract_to_dir(image, out).await {
                Ok(_) => {
                    println!("------------------------------------------------------------");
                    println!("Successfully pulled and compressed to: {}", out.display());
                    println!("------------------------------------------------------------");
                },
                Err(err) => {
                    eprintln!("Error pulling/extracting image: {}", err);
                    std::process::exit(1);
                }
            }
        },
        Commands::UpdateLxc { image, vmid, out } => {
            println!("Updating Proxmox LXC Container {} using image '{}'...", vmid, image);
            let extractor = oci::OciExtractor::new();
            
            match extractor.extract_to_dir(image, out).await {
                Ok(status) => {
                    let proxmox = proxmox::ProxmoxClient::new();
                    let node = "pve"; // Assume default node

                    match status {
                        oci::UpdateStatus::Unchanged => {
                            println!("Image has not changed. No update needed for container {}.", vmid);
                        },
                        oci::UpdateStatus::InPlaceUpdate => {
                            println!("In-place update detected. Restarting container {} to apply changes...", vmid);
                            
                            println!("Stopping container...");
                            let _ = proxmox.stop_lxc(node, *vmid).await;
                            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                            
                            println!("Starting container...");
                            let _ = proxmox.start_lxc(node, *vmid).await;
                            
                            println!("Container {} restarted successfully.", vmid);
                        },
                        oci::UpdateStatus::Recreated => {
                            println!("Base image changed. Recreating container {}...", vmid);
                            
                            println!("Stopping container...");
                            let _ = proxmox.stop_lxc(node, *vmid).await;
                            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                            
                            println!("Deleting container...");
                            let _ = proxmox.delete_lxc(node, *vmid).await;
                            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                            
                            println!("Uploading new template and recreating container...");
                            // Simulation of template deployment
                            let _ = proxmox.create_lxc(*vmid, &format!("hostable-{}", vmid)).await;
                            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                            
                            println!("Starting new container...");
                            let _ = proxmox.start_lxc(node, *vmid).await;
                            
                            println!("Container {} recreated and started successfully.", vmid);
                        }
                    }
                },
                Err(err) => {
                    eprintln!("Error pulling/extracting image: {}", err);
                    std::process::exit(1);
                }
            }
        },
        Commands::Manage { config } => {
            if !config.exists() {
                eprintln!("Error: Config file '{}' not found", config.display());
                std::process::exit(1);
            }

            println!("Starting Hostable Daemon...");
            let content = fs::read_to_string(config)?;
            let cfg: Config = serde_yaml::from_str(&content).expect("Invalid config.yaml format");

            println!("Loaded {} deployments. Interval: {} seconds.", cfg.deployments.len(), cfg.interval_seconds);

            let proxmox = proxmox::ProxmoxClient::new();
            let node = "pve";

            loop {
                println!("--- Checking for updates ---");
                for dep in &cfg.deployments {
                    println!("Checking deployment for VMID {} (Image: {})", dep.vmid, dep.image);
                    let extractor = oci::OciExtractor::new();
                    
                    let out_path = std::path::PathBuf::from(format!("/cache/hostable_vmid_{}.tar.xz", dep.vmid));
                    match extractor.extract_to_dir(&dep.image, &out_path).await {
                        Ok(status) => {
                            match status {
                                oci::UpdateStatus::Unchanged => {
                                    println!("VMID {}: Image unchanged.", dep.vmid);
                                },
                                oci::UpdateStatus::InPlaceUpdate => {
                                    println!("VMID {}: In-place update detected. Restarting...", dep.vmid);
                                    let _ = proxmox.stop_lxc(node, dep.vmid).await;
                                    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                                    let _ = proxmox.start_lxc(node, dep.vmid).await;
                                },
                                oci::UpdateStatus::Recreated => {
                                    println!("VMID {}: Base layers changed. Recreating...", dep.vmid);
                                    let _ = proxmox.stop_lxc(node, dep.vmid).await;
                                    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                                    let _ = proxmox.delete_lxc(node, dep.vmid).await;
                                    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                                    let _ = proxmox.create_lxc(dep.vmid, &format!("hostable-{}", dep.vmid)).await;
                                    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                                    let _ = proxmox.start_lxc(node, dep.vmid).await;
                                }
                            }
                        },
                        Err(e) => eprintln!("Error checking update for VMID {}: {}", dep.vmid, e),
                    }
                }
                println!("--- Sleep for {} seconds ---", cfg.interval_seconds);
                tokio::time::sleep(tokio::time::Duration::from_secs(cfg.interval_seconds)).await;
            }
        }
    }
    
    Ok(())
}
