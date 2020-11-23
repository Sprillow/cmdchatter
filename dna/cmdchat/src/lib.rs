use hdk3::prelude::*;

pub const AGENTS_PATH: &str = "agents";
pub const MESSAGES_PATH: &str = "messages";

#[hdk_extern]
fn init(_: ()) -> ExternResult<InitCallbackResult> {
    let messages_path = Path::from(MESSAGES_PATH);
    messages_path.ensure()?;

    let agents_path = Path::from(AGENTS_PATH);
    agents_path.ensure()?;
    let agents_path_address = agents_path.hash()?;
    let agent = agent_info()?.agent_initial_pubkey;
    create_entry(&Agent(agent.clone()))?;
    let agent_entry_hash = hash_entry(&Agent(agent.clone()))?;
    create_link(agents_path_address, agent_entry_hash, ())?;

    let mut functions: GrantedFunctions = HashSet::new();
    functions.insert((zome_info()?.zome_name, "receive_signal".into()));
    create_cap_grant(CapGrantEntry {
        tag: "".into(),
        // empty access converts to unrestricted
        access: ().into(),
        functions,
    })?;

    Ok(InitCallbackResult::Pass)
}

entry_defs![Path::entry_def(), Message::entry_def(), Agent::entry_def()];

#[hdk_entry(id = "message")]
#[derive(Debug, Clone, PartialEq)]
pub struct Message(pub String, pub AgentPubKey);

// #[derive(Debug, Serialize, Deserialize, SerializedBytes)]
// pub struct MessageSignal(Message, AgentPubKey);

#[hdk_entry(id = "agent")]
#[derive(Debug, Clone, PartialEq)]
pub struct Agent(AgentPubKey);

pub fn signal_peers(signal: &Message) -> ExternResult<()> {
    // peers
    let peers = get_peers()?;
    let zome_info = zome_info()?;
    let _ = debug!(format!("PEERS! {:?}", peers));
    for peer in peers {
        let res: HdkResult<()> = call_remote(
            peer,
            zome_info.zome_name.clone(),
            zome::FunctionName("receive_signal".into()),
            None,
            signal,
        );
        if res.is_err() {
            let _ = debug!(format!("Error during signal_peers {:?}", res.unwrap_err()));
        }
    }
    Ok(())
}

// used to get addresses of agents to send signals to
fn get_peers() -> ExternResult<Vec<AgentPubKey>> {
    let path_hash = Path::from(AGENTS_PATH).hash()?;

    let entries = get_links(path_hash, None)?
        .into_inner()
        .into_iter()
        .map(|link: link::Link| get(link.target, GetOptions))
        .filter_map(Result::ok)
        .filter_map(|maybe_el| maybe_el)
        .map(|el| el.entry().to_app_option::<Agent>())
        .filter_map(Result::ok)
        .filter_map(|maybe_agent| maybe_agent)
        .collect::<Vec<Agent>>();

    let agent_info = agent_info()?;
    Ok(entries
        .into_iter()
        // eliminate yourself as a peer
        .filter(|x| x.0 != agent_info.agent_initial_pubkey)
        .map(|x| x.0)
        .collect::<Vec<AgentPubKey>>())
}

#[hdk_extern]
pub fn create_message(message: Message) -> ExternResult<()> {
    let _address = create_entry(&message)?;
    let entry_hash = hash_entry(&message)?;
    let path_hash = Path::from(MESSAGES_PATH).hash()?;
    create_link(path_hash, entry_hash.clone(), ())?;

    // let agent = agent_info()?.agent_initial_pubkey;
    let _ = debug!(format!("CREATE ACTION SIGNAL PEERS {:?}", message));
    let _ = signal_peers(&message);
    Ok(())
}

#[derive(Serialize, Deserialize, SerializedBytes)]
pub struct FetchMessagesResponse(Vec<Message>);

#[hdk_extern]
pub fn fetch_messages(_: ()) -> ExternResult<FetchMessagesResponse> {
    let path_hash = Path::from(MESSAGES_PATH).hash()?;

    let entries = get_links(path_hash, None)?
        .into_inner()
        .into_iter()
        .map(|link: link::Link| get(link.target, GetOptions))
        .filter_map(Result::ok)
        .filter_map(|maybe_el| maybe_el)
        .map(|el| el.entry().to_app_option::<Message>())
        .filter_map(Result::ok)
        .filter_map(|maybe_msg| maybe_msg)
        .collect::<Vec<Message>>();

    Ok(FetchMessagesResponse(entries))
}

// receiver (and forward to UI)
#[hdk_extern]
pub fn receive_signal(signal: Message) -> ExternResult<()> {
    match emit_signal(&signal) {
        Ok(_) => Ok(()),
        Err(_) => Err(HdkError::SerializedBytes(SerializedBytesError::ToBytes(
            "couldnt convert to bytes to send as signal".to_string(),
        ))),
    }
}
