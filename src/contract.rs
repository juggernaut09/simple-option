use cosmwasm_std::{
    to_binary, Api, BankMsg, Binary, Context, Env, Extern, HandleResponse, HumanAddr, InitResponse,
    MessageInfo, Querier, StdError, StdResult, Storage,
};

use crate::msg::{ConfigResponse, HandleMsg, InitMsg, QueryMsg};
use crate::state::{config, config_read, State};

// Note, you can use StdResult in some functions where you do not
// make use of the custom errors
pub fn init<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    info: MessageInfo,
    msg: InitMsg,
) -> StdResult<InitResponse> {
    if msg.expires <= env.block.height {
        return Err(StdError::generic_err("Cannot create expired option"));
    }

    let state = State {
        creator: info.sender.clone(),
        owner: info.sender.clone(),
        collateral: info.sent_funds,
        counter_offer: msg.counter_offer,
        expires: msg.expires,
    };
    config(&mut deps.storage).save(&state)?;

    Ok(InitResponse::default())
}

// And declare a custom Error variant for the ones where you will want to make use of it
pub fn handle<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    info: MessageInfo,
    env: Env,
    msg: HandleMsg,
) -> StdResult<HandleResponse> {
    match msg {
        HandleMsg::Transfer { recipient } => handle_transfer(deps, info, recipient),
        HandleMsg::Execute {} => handle_execute(deps, info, env),
        HandleMsg::Burn {} => handle_burn(deps, info, env),
    }
}
pub fn handle_transfer<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    info: MessageInfo,
    recipient: HumanAddr,
) -> StdResult<HandleResponse> {
    let mut state: State = config(&mut deps.storage).load()?;

    // ensure msg.sender is the owner
    if info.sender != state.owner {
        return Err(StdError::generic_err("Sender must be owner"));
    }

    // set ne owner on state
    state.owner = recipient.clone();
    config(&mut deps.storage).save(&state)?;

    let mut res = Context::new();
    res.add_attribute("action", "transfer");
    res.add_attribute("owner", recipient);
    Ok(res.into())
}

pub fn handle_execute<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    info: MessageInfo,
    env: Env,
) -> StdResult<HandleResponse> {
    // ensure message sender is the owner
    let state: State = config(&mut deps.storage).load()?;
    if info.sender != state.owner {
        return Err(StdError::generic_err("Sender must be owner"));
    }

    // ensure not expired
    if env.block.height >= state.expires {
        return Err(StdError::generic_err("option expired"));
    }
    // ensure sending proper counter_offer
    if info.sent_funds != state.counter_offer {
        return Err(StdError::generic_err(format!(
            "must send exact counter_offer: {:?}",
            state.counter_offer
        )));
    }
    // release counter_offer to creator
    let mut res = Context::new();
    res.add_message(BankMsg::Send {
        from_address: env.contract.address.clone(),
        to_address: state.creator,
        amount: state.counter_offer,
    });

    // release collateral to sender
    res.add_message(BankMsg::Send {
        from_address: env.contract.address,
        to_address: state.owner,
        amount: state.collateral,
    });

    // delete the option
    config(&mut deps.storage).remove();

    res.add_attribute("action", "execute");
    Ok(res.into())
}

pub fn handle_burn<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    info: MessageInfo,
    env: Env,
) -> StdResult<HandleResponse> {
    let state: State = config(&mut deps.storage).load()?;
    // ensure is expired
    if env.block.height < state.expires {
        return Err(StdError::generic_err("option not yet expired"));
    }

    // ensure not sending the counter_offer
    if !info.sent_funds.is_empty() {
        return Err(StdError::generic_err("don't send funds with burn"));
    }

    // release collateral to creator
    let mut res = Context::new();
    res.add_message(BankMsg::Send {
        from_address: env.contract.address,
        to_address: state.creator,
        amount: state.collateral,
    });

    // delete the option
    config(&mut deps.storage).remove();
    res.add_attribute("action", "burn");
    Ok(res.into())
}

pub fn query<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
    _env: Env,
    msg: QueryMsg,
) -> StdResult<Binary> {
    match msg {
        QueryMsg::Config {} => to_binary(&query_config(deps)?),
    }
}

fn query_config<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
) -> StdResult<ConfigResponse> {
    let state = config_read(&deps.storage).load()?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::testing::{mock_dependencies, mock_env, mock_info, MOCK_CONTRACT_ADDR};
    use cosmwasm_std::{coins, attr, CosmosMsg};

    #[test]
    fn proper_initialization() {
        let msg = InitMsg {
            counter_offer: coins(40, "ETH"),
            expires: 100_000,
        };
        let mut deps = mock_dependencies(&[]);
        let info = mock_info("creator", &coins(1, "BTC"));
        let env = mock_env();

        // we can jut call .unwrap() to assert this was a success
        let res = init(&mut deps, env, info, msg).unwrap();
        assert_eq!(0, res.messages.len());

        // It worked, let's query the state
        let res = query_config(&deps).unwrap();
        assert_eq!(100_000, res.expires);
        assert_eq!("creator", res.owner.as_str());
        assert_eq!("creator", res.creator.as_str());
        assert_eq!(coins(1, "BTC"), res.collateral);
        assert_eq!(coins(40, "ETH"), res.counter_offer);

    }

    #[test]
    fn transfer() {
        let mut deps = mock_dependencies(&[]);
        let msg = InitMsg {
            counter_offer: coins(40, "ETH"),
            expires: 100_000,
        };
        let env = mock_env();
        let info = mock_info("creator", &coins(1, "BTC"));

        let res = init(&mut deps, env, info, msg).unwrap();
        assert_eq!(0, res.messages.len());

        // random cannot transfer
        let info = mock_info("anyone", &[]);
        let err = handle_transfer(&mut deps, info, HumanAddr::from("anyone")).unwrap_err();
        match err {
            StdError::GenericErr { .. } => {}
            e => panic!("unexpected error: {}", e),
        }

        // owner can transfer
        let info = mock_info("creator", &[]);
        let res = handle_transfer(&mut deps,info, HumanAddr::from("someone")).unwrap();
        assert_eq!(res.attributes.len(), 2);
        assert_eq!(res.attributes[0], attr("action", "transfer"));

        // check updated properly
        let res = query_config(&deps).unwrap();
        assert_eq!(100_000, res.expires);
        assert_eq!("someone", res.owner.as_str());
        assert_eq!("creator", res.creator.as_str());
        assert_eq!(coins(1, "BTC"), res.collateral);
        assert_eq!(coins(40, "ETH"), res.counter_offer);

    }

    #[test]
    fn execute() {
        let mut deps = mock_dependencies(&[]);

        let counter_offer = coins(40, "ETH");
        let collateral = coins(1, "BTC");
        let msg = InitMsg {
            counter_offer: counter_offer.clone(),
            expires: 100_000
        };
        let info = mock_info("creator", &collateral);

        let _ = init(&mut deps, mock_env(), info, msg).unwrap();

        // set a new owner
        let info = mock_info("creator", &[]);
        let _ = handle_transfer(&mut deps, info, HumanAddr::from("owner")).unwrap();

        // random person cannot execute
        let info = mock_info("anyone", &counter_offer);
        let err = handle_execute(&mut deps, info, mock_env()).unwrap_err();
        match err {
            StdError::GenericErr { msg,.. } => assert_eq!("Sender must be owner", msg.as_str()),
            e => panic!("unexpected error : {}", e),
        }

        // expired cannot execute
        let info = mock_info("owner", &counter_offer);
        let mut env = mock_env();
        env.block.height = 200_000;
        let err = handle_execute(&mut deps, info, env).unwrap_err();
        match err {
            StdError::GenericErr { msg, .. } => {
                assert_eq!("option expired", msg.as_str())
            },
            e => panic!("unexpected error: {}", e),
        }

        // bad counter_offer cannot execute
        let info = mock_info("owner", &coins(39, "ETH"));
        let err = handle_execute(&mut deps, info, mock_env()).unwrap_err();
        match err {
            StdError::GenericErr {msg, ..} => assert_eq!(format!("must send exact counter_offer: {:?}", &counter_offer), msg.as_str()),
            e => panic!("unexpected error : {}", e),
        }


        // proper execution
        let info = mock_info("owner", &counter_offer);
        let res = handle_execute(&mut deps, info, mock_env()).unwrap();
        assert_eq!(res.messages.len(), 2);
        assert_eq!(res.messages[0], CosmosMsg::Bank(BankMsg::Send {
            from_address: MOCK_CONTRACT_ADDR.into(),
            to_address: "creator".into(),
            amount: counter_offer,
        }));
        assert_eq!(res.messages[1], CosmosMsg::Bank(BankMsg::Send {
            from_address: MOCK_CONTRACT_ADDR.into(),
            to_address: "owner".into(),
            amount: collateral,
        }));

        // check deleted
        let _ = query_config(&deps).unwrap_err();



    }

    #[test]
    fn burn() {
        let mut deps = mock_dependencies(&[]);

        let counter_offer = coins(40, "ETH");
        let collateral = coins(1, "BTC");
        let msg = InitMsg {
            counter_offer: counter_offer.clone(),
            expires: 100_000
        };
        let info = mock_info("creator", &collateral);

        let _ = init(&mut deps, mock_env(), info, msg).unwrap();

        // set a new owner
        let info = mock_info("creator", &[]);
        let _ = handle_transfer(&mut deps, info, HumanAddr::from("owner")).unwrap();

        // non-expired cannot execute
        let info = mock_info("owner", &counter_offer);
        let err = handle_burn(&mut deps, info, mock_env()).unwrap_err();
        match err {
            StdError::GenericErr { msg, .. } => {
                assert_eq!("option not yet expired", msg.as_str())
            },
            e => panic!("unexpected error: {}", e),
        }

        // with funds cannot execute
        let info = mock_info("owner", &counter_offer);
        let mut env = mock_env();
        env.block.height = 200_000;
        let err = handle_burn(&mut deps, info, env).unwrap_err();
        match err {
            StdError::GenericErr { msg, .. } => {
                assert_eq!("don't send funds with burn", msg.as_str())
            }
            e => panic!("unexpected error: {}", e),
        }

        // expired returns funds
        let info = mock_info("owner", &[]);
        let mut env = mock_env();
        env.block.height = 200_000;
        let res = handle_burn(&mut deps, info, env).unwrap();
        assert_eq!(res.messages.len(), 1);
        assert_eq!(
            res.messages[0],
            CosmosMsg::Bank(BankMsg::Send {
                from_address: MOCK_CONTRACT_ADDR.into(),
                to_address: "creator".into(),
                amount: collateral,
            })
        );

        // check deleted
        let _ = query_config(&deps).unwrap_err();
    }
}
