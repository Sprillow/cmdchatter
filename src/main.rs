use hdk3::prelude::{holochain_serial, AgentPubKey, Deserialize, Serialize};
use holochain::{
    conductor::{
        api::error::{ConductorApiError, ConductorApiResult},
        config::{ConductorConfig, PassphraseServiceConfig},
        error::CreateAppError,
        CellError, Conductor, ConductorHandle,
    },
    core::{ribosome::ZomeCallInvocation, signal::Signal, workflow::ZomeCallInvocationResult},
};
use holochain_keystore::KeystoreSenderExt;
use holochain_p2p::kitsune_p2p::{KitsuneP2pConfig, ProxyConfig, TransportConfig, dependencies::url2::{self, Url2}};
use holochain_types::{
    app::{CellNick, InstalledAppId, InstalledCell},
    cell::CellId,
    dna::DnaFile,
    observability::{self, Output},
    prelude::SerializedBytes,
};
use holochain_zome_types::{zome, ExternInput, ZomeCallResponse};
use rustyline::error::ReadlineError;
use rustyline::Editor;
use std::{convert::TryFrom, path::PathBuf};
use std::{error::Error, path::Path};
use structopt::StructOpt;
use tokio::stream::StreamExt;
use tracing::*;

const INSTALLED_APP_ID: &str = "my_app_id";

const DATABASES_PATH: &'static str = "./databases";
const CMDCHAT: &'static [u8] = include_bytes!("../dna/cmdchat.dna.gz");

#[derive(Debug, StructOpt)]
#[structopt(
    name = "cmdchatter",
    about = "chat system over command line with holochain"
)]
struct Opt {}

fn main() {
    holochain::conductor::tokio_runtime()
        // the async_main function should only end if our program is done
        .block_on(async_main())
}

// matches the inside... could be a shared struct
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerializedBytes)]
pub struct Message(pub String, pub AgentPubKey);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerializedBytes)]
pub struct FetchMessagesResponse(Vec<Message>);

async fn async_main() {
    // Sets up a human-readable panic message with a request for bug reports
    //
    // See https://docs.rs/human-panic/1.0.3/human_panic/
    human_panic::setup_panic!();

    // take in command line arguments
    let _opt = Opt::from_args();

    observability::init_fmt(Output::Log).expect("Failed to start contextual logging");
    debug!("observability initialized");

    if !Path::new(DATABASES_PATH).exists() {
        if let Err(e) = std::fs::create_dir(DATABASES_PATH) {
            error!("{}", e);
            panic!()
        };
    }

    let conductor = conductor_handle().await;

    let cell_id = match install_or_passthrough(&conductor).await {
        Err(e) => {
            error!("{:?}", e);
            panic!();
        }
        Ok(cell_id) => cell_id,
    };

    println!("fetching message history... please wait");
    let res = zome_call(
        &conductor,
        cell_id.clone(),
        "fetch_messages",
        SerializedBytes::default(),
    )
    .await;
    if let Ok(Ok(ZomeCallResponse::Ok(eo))) = res {
        match FetchMessagesResponse::try_from(eo.into_inner()) {
            Ok(f) => f.0.into_iter().for_each(|message| {
                display_message(message);
            }),
            Err(e) => {
                println!("{}", e);
            }
        }
    }

    match conductor.clone().add_signal_channel().await {
        Ok(mut receiver) => {
            tokio::task::spawn(async move {
                loop {
                    if let Some(Ok(signal)) = receiver.next().await {
                        if let Signal::App(_, bytes) = signal {
                            match Message::try_from(bytes) {
                                Ok(msg) => {
                                    display_message(msg);
                                }
                                Err(e) => {
                                    println!("err {}", e);
                                }
                            }
                        }
                    } else {
                        debug!("Closing interface: signal stream empty");
                        break;
                    }
                }
            });
        }
        Err(e) => {
            println!("error while attaching signal listener {}", e);
            panic!();
        }
    };

    /* run the readline loop */
    println!("type anything and hit enter to send a message");
    let mut rl = Editor::<()>::new();
    // first five chars are identical for all
    let name = format!("{}: ", &cell_id.agent_pubkey().to_string()[5..10]);
    loop {
        let readline = rl.readline(name.as_str());
        match readline {
            Ok(line) => {
                // rl.add_history_entry(line.as_str());
                let message = Message(line, cell_id.agent_pubkey().clone());
                match SerializedBytes::try_from(message.clone()) {
                    Ok(sb) => {
                        // todo: this error handling isn't sufficient
                        // there are inner errors possibly
                        let res =
                            zome_call(&conductor, cell_id.clone(), "create_message", sb).await;
                        if res.is_err() {
                            println!("{:?}", res);
                        }
                    }
                    Err(_) => {}
                };
            }
            Err(ReadlineError::Interrupted) => {
                println!("CTRL-C");
                break;
            }
            Err(ReadlineError::Eof) => {
                println!("CTRL-D");
                break;
            }
            Err(err) => {
                println!("Error: {:?}", err);
                break;
            }
        }
    }

    // Await on the main JoinHandle, keeping the process alive until all
    // Conductor activity has ceased
    conductor
        .take_shutdown_handle()
        .await
        .expect("The shutdown handle has already been taken.")
        .await
        .map_err(|e| {
            error!(error = &e as &dyn Error, "Failed to join the main task");
            e
        })
        .expect("Error while joining threads during shutdown");
    // TODO: on SIGINT/SIGKILL, kill the conductor:
    // conductor.kill().await
}

fn display_message(message: Message) {
    // first five chars are identical for all
    println!("{}: {}", &message.1.to_string()[5..10], message.0);
}

async fn zome_call(
    conductor: &ConductorHandle,
    cell_id: CellId,
    zome_name: &str,
    sb: SerializedBytes,
) -> ConductorApiResult<ZomeCallInvocationResult> {
    let request = ZomeCallInvocation {
        cell_id: cell_id.clone(),
        zome_name: zome::ZomeName::from("cmdchat"),
        cap: None,
        fn_name: zome::FunctionName::from(zome_name),
        payload: ExternInput::new(sb),
        provenance: cell_id.agent_pubkey().clone(),
    };
    conductor.call_zome(request).await
}

async fn conductor_handle() -> ConductorHandle {
    // const ADMIN_PORT: u16 = 1234;

    let mut network_config = KitsuneP2pConfig::default();
    network_config.bootstrap_service = Some(url2::url2!("https://bootstrap.holo.host"));
    network_config.transport_pool.push(TransportConfig::Proxy {
        sub_transport: Box::new(TransportConfig::Quic {
            bind_to: Some(url2::url2!("kitsune-quic://0.0.0.0:0")),
            override_host: None,
            override_port: None,
        }),
        // proxy_config: ProxyConfig::LocalProxyServer {
        //     proxy_accept_config: Some(ProxyAcceptConfig::RejectAll),
        // },
        proxy_config: ProxyConfig::RemoteProxyClient {
          proxy_url: Url2::parse("kitsune-proxy://CIW6PxKxsPPlcuvUCbMcKwUpaMSmB7kLD8xyyj4mqcw/kitsune-quic/h/proxy.holochain.org/p/5778/--")
        }
    });

    /*
    Some(vec![AdminInterfaceConfig {
            driver: InterfaceDriver::Websocket { port: ADMIN_PORT },
        }])
    */
    let config: ConductorConfig = ConductorConfig {
        environment_path: PathBuf::from(DATABASES_PATH).into(),
        use_dangerous_test_keystore: false,
        signing_service_uri: None,
        encryption_service_uri: None,
        decryption_service_uri: None,
        dpki: None,
        passphrase_service: Some(PassphraseServiceConfig::Cmd),
        keystore_path: None,
        admin_interfaces: None,
        network: Some(network_config),
    };

    // Initialize the Conductor
    Conductor::builder()
        .config(config)
        .build()
        .await
        .expect("Could not initialize Conductor from configuration")
}

async fn install_or_passthrough(conductor: &ConductorHandle) -> ConductorApiResult<CellId> {
    let cell_ids = conductor.list_cell_ids().await?;
    let cell_id = match cell_ids.len() {
        0 => {
            println!("Don't see existing files or identity, so starting fresh...");
            let cell_id = install_app(&conductor).await?;
            println!("Installed, now activating...");
            activate_app(&conductor).await?;
            println!("Activated.");
            cell_id
        }
        _ => cell_ids.first().unwrap().clone(),
    };
    // HISTORICAL, we had to do it this way, without
    // having had a way to call add_signal_channel
    // let _port = conductor.clone().add_app_interface(0).await?;
    Ok(cell_id)
}

async fn install_app(conductor_handle: &ConductorHandle) -> ConductorApiResult<CellId> {
    println!("Don't recognize you, so generating a new identity for you...");
    let agent_key = conductor_handle
        .keystore()
        .clone()
        .generate_sign_keypair_from_pure_entropy()
        .await?;

    // Our dna
    let dnas: Vec<(Vec<u8>, CellNick)> = vec![(CMDCHAT.into(), "cmdchat".to_string())];

    let tasks = dnas.into_iter().map(|(dna_bytes, nick)| async {
        let dna = read_parse_dna(dna_bytes).await?;
        let hash = dna.dna_hash().clone();
        let cell_id = CellId::from((hash.clone(), agent_key.clone()));
        conductor_handle.install_dna(dna).await?;
        ConductorApiResult::Ok((InstalledCell::new(cell_id, nick), None))
    });

    // Join all the install tasks
    let cell_ids_with_proofs = futures::future::join_all(tasks)
        .await
        .into_iter()
        // Check all passed and return the proofs
        .collect::<Result<Vec<_>, _>>()?;

    let installed_app_id: InstalledAppId = String::from(INSTALLED_APP_ID);
    // Call genesis
    conductor_handle
        .clone()
        .install_app(installed_app_id, cell_ids_with_proofs.clone())
        .await?;

    Ok(cell_ids_with_proofs[0].clone().0.into_id())
}

async fn activate_app(conductor_handle: &ConductorHandle) -> ConductorApiResult<()> {
    // Activate app
    let installed_app_id: InstalledAppId = String::from(INSTALLED_APP_ID);
    conductor_handle
        .activate_app(installed_app_id.clone())
        .await?;

    // Create cells
    let errors = conductor_handle.clone().setup_cells().await?;

    // Check if this app was created successfully
    errors
        .into_iter()
        // We only care about this app for the activate command
        .find(|cell_error| match cell_error {
            CreateAppError::Failed {
                installed_app_id: error_app_id,
                ..
            } => error_app_id == &installed_app_id,
        })
        // There was an error in this app so return it
        .map(|this_app_error| {
            let CreateAppError::Failed { errors: ee, .. } = this_app_error;
            let b = ee[0].to_string();
            error!("{:?}", b);
            // TODO -> this was annoying because I couldn't Copy the
            // real CellError
            Err(ConductorApiError::CellError(CellError::Todo))
        })
        // No error, return success
        .unwrap_or(Ok(()))
}

async fn read_parse_dna(dna_bytes: Vec<u8>) -> ConductorApiResult<DnaFile> {
    let dna = DnaFile::from_file_content(&dna_bytes).await?;
    Ok(dna)
}
