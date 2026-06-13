#![warn(unused_crate_dependencies)]

use async_nats::service::ServiceExt;
use bytes::Bytes;
use clap::Parser;
use tokio_stream::StreamExt;

#[derive(Parser)]
#[command(version, about = "Starfish daemon")]
struct Cli {}

#[tokio::main]
async fn main() -> Result<(), async_nats::Error> {
    let _cli = Cli::parse();
    // Connect to the NATS server
    let client = async_nats::connect("nats://localhost:4222").await?;

    // Create and start the service
    let service = client
        .service_builder()
        .description("A simple min/max service")
        .start("minmax", "0.1.0")
        .await?;

    // Define the min endpoint
    let group = service.group("v1");
    let mut min = group.endpoint("min").await?;

    // Handle incoming requests
    tokio::spawn(async move {
        while let Some(request) = min.next().await {
            let input: Vec<i32> = serde_json::from_slice(&request.message.payload).unwrap();
            let result = input.iter().min().unwrap();
            request
                .respond(Ok(Bytes::copy_from_slice(&result.to_be_bytes())))
                .await
                .unwrap();
        }
    });

    // Define the max endpoint
    let mut max = group.endpoint("max").await?;
    tokio::spawn(async move {
        while let Some(request) = max.next().await {
            let input: Vec<i32> = serde_json::from_slice(&request.message.payload).unwrap();
            let result = input.iter().max().unwrap();
            request
                .respond(Ok(Bytes::copy_from_slice(&result.to_be_bytes())))
                .await
                .unwrap();
        }
    });

    // Keep the service running
    tokio::signal::ctrl_c().await?;
    Ok(())
}
