use serde::de::DeserializeOwned;
use serde::Serialize;

use cosmwasm_std::{
    to_binary, Addr, Binary, BlockInfo, CustomMsg, Deps, Env, Order, StdError, StdResult,
};

use cw721::{
    AllNftInfoResponse, ApprovalResponse, ApprovalsResponse, ContractInfoResponse, Cw721Query,
    Expiration, NftInfoResponse, NumTokensResponse, OperatorResponse, OperatorsResponse,
    OwnerOfResponse, TokensResponse,
};
use cw_storage_plus::Bound;
use cw_utils::maybe_addr;

use crate::msg::{MinterResponse, QueryMsg};
use crate::state::{Approval, Cw721Contract, TokenInfo};

const DEFAULT_LIMIT: u32 = 10;
const MAX_LIMIT: u32 = 100;

impl<'a, T, C, E, Q> Cw721Query<T> for Cw721Contract<'a, T, C, E, Q>
where
    T: Serialize + DeserializeOwned + Clone,
    C: CustomMsg,
    E: CustomMsg,
    Q: CustomMsg,
{
    fn contract_info(&self, deps: Deps) -> StdResult<ContractInfoResponse> {
        self.contract_info.load(deps.storage)
    }

    fn num_tokens(&self, deps: Deps) -> StdResult<NumTokensResponse> {
        let count = self.token_count(deps.storage)?;
        Ok(NumTokensResponse { count })
    }

    fn nft_info(&self, deps: Deps, token_id: String) -> StdResult<NftInfoResponse<T>> {
        let info = self.tokens.load(deps.storage, &token_id)?;
        Ok(NftInfoResponse {
            token_uri: info.token_uri,
            extension: info.extension,
        })
    }

    fn owner_of(
        &self,
        deps: Deps,
        env: Env,
        token_id: String,
        include_expired: bool,
    ) -> StdResult<OwnerOfResponse> {
        let info = self.tokens.load(deps.storage, &token_id)?;
        Ok(OwnerOfResponse {
            owner: info.owner.to_string(),
            approvals: humanize_approvals(&env.block, &info, include_expired),
        })
    }

    /// operator returns the approval status of an operator for a given owner if exists
    fn operator(
        &self,
        deps: Deps,
        env: Env,
        owner: String,
        operator: String,
        include_expired: bool,
    ) -> StdResult<OperatorResponse> {
        let owner_addr = deps.api.addr_validate(&owner)?;
        let operator_addr = deps.api.addr_validate(&operator)?;

        let info = self
            .operators
            .may_load(deps.storage, (&owner_addr, &operator_addr))?;

        if let Some(expires) = info {
            if !include_expired && expires.is_expired(&env.block) {
                return Err(StdError::not_found("Approval not found"));
            }

            return Ok(OperatorResponse {
                approval: cw721::Approval {
                    spender: operator,
                    expires,
                },
            });
        }

        Err(StdError::not_found("Approval not found"))
    }

    /// operators returns all operators owner given access to
    fn operators(
        &self,
        deps: Deps,
        env: Env,
        owner: String,
        include_expired: bool,
        start_after: Option<String>,
        limit: Option<u32>,
    ) -> StdResult<OperatorsResponse> {
        let limit = limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;
        let start_addr = maybe_addr(deps.api, start_after)?;
        let start = start_addr.as_ref().map(Bound::exclusive);

        let owner_addr = deps.api.addr_validate(&owner)?;
        let res: StdResult<Vec<_>> = self
            .operators
            .prefix(&owner_addr)
            .range(deps.storage, start, None, Order::Ascending)
            .filter(|r| {
                include_expired || r.is_err() || !r.as_ref().unwrap().1.is_expired(&env.block)
            })
            .take(limit)
            .map(parse_approval)
            .collect();
        Ok(OperatorsResponse { operators: res? })
    }

    fn approval(
        &self,
        deps: Deps,
        env: Env,
        token_id: String,
        spender: String,
        include_expired: bool,
    ) -> StdResult<ApprovalResponse> {
        let token = self.tokens.load(deps.storage, &token_id)?;

        // token owner has absolute approval
        if token.owner == spender {
            let approval = cw721::Approval {
                spender: token.owner.to_string(),
                expires: Expiration::Never {},
            };
            return Ok(ApprovalResponse { approval });
        }

        let filtered: Vec<_> = token
            .approvals
            .into_iter()
            .filter(|t| t.spender == spender)
            .filter(|t| include_expired || !t.is_expired(&env.block))
            .map(|a| cw721::Approval {
                spender: a.spender.into_string(),
                expires: a.expires,
            })
            .collect();

        if filtered.is_empty() {
            return Err(StdError::not_found("Approval not found"));
        }
        // we expect only one item
        let approval = filtered[0].clone();

        Ok(ApprovalResponse { approval })
    }

    /// approvals returns all approvals owner given access to
    fn approvals(
        &self,
        deps: Deps,
        env: Env,
        token_id: String,
        include_expired: bool,
    ) -> StdResult<ApprovalsResponse> {
        let token = self.tokens.load(deps.storage, &token_id)?;
        let approvals: Vec<_> = token
            .approvals
            .into_iter()
            .filter(|t| include_expired || !t.is_expired(&env.block))
            .map(|a| cw721::Approval {
                spender: a.spender.into_string(),
                expires: a.expires,
            })
            .collect();

        Ok(ApprovalsResponse { approvals })
    }

    fn tokens(
        &self,
        deps: Deps,
        owner: String,
        start_after: Option<String>,
        limit: Option<u32>,
    ) -> StdResult<TokensResponse> {
        let limit = limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;
        let start = start_after.map(|s| Bound::ExclusiveRaw(s.into()));

        let owner_addr = deps.api.addr_validate(&owner)?;
        let tokens: Vec<String> = self
            .tokens
            .idx
            .owner
            .prefix(owner_addr)
            .keys(deps.storage, start, None, Order::Ascending)
            .take(limit)
            .collect::<StdResult<Vec<_>>>()?;

        Ok(TokensResponse { tokens })
    }

    fn all_tokens(
        &self,
        deps: Deps,
        start_after: Option<String>,
        limit: Option<u32>,
    ) -> StdResult<TokensResponse> {
        let limit = limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;
        let start = start_after.map(|s| Bound::ExclusiveRaw(s.into()));

        let tokens: StdResult<Vec<String>> = self
            .tokens
            .range(deps.storage, start, None, Order::Ascending)
            .take(limit)
            .map(|item| item.map(|(k, _)| k))
            .collect();

        Ok(TokensResponse { tokens: tokens? })
    }

    fn all_nft_info(
        &self,
        deps: Deps,
        env: Env,
        token_id: String,
        include_expired: bool,
    ) -> StdResult<AllNftInfoResponse<T>> {
        let info = self.tokens.load(deps.storage, &token_id)?;
        Ok(AllNftInfoResponse {
            access: OwnerOfResponse {
                owner: info.owner.to_string(),
                approvals: humanize_approvals(&env.block, &info, include_expired),
            },
            info: NftInfoResponse {
                token_uri: info.token_uri,
                extension: info.extension,
            },
        })
    }
}

impl<'a, T, C, E, Q> Cw721Contract<'a, T, C, E, Q>
where
    T: Serialize + DeserializeOwned + Clone,
    C: CustomMsg,
    E: CustomMsg,
    Q: CustomMsg,
{
    pub fn query(&self, deps: Deps, env: Env, msg: QueryMsg<Q>) -> StdResult<Binary> {
        match msg {
            QueryMsg::Minter {} => to_binary(&self.minter(deps)?),
            QueryMsg::ContractInfo {} => to_binary(&self.contract_info(deps)?),
            QueryMsg::NftInfo { token_id } => to_binary(&self.nft_info(deps, token_id)?),
            QueryMsg::OwnerOf {
                token_id,
                include_expired,
            } => {
                to_binary(&self.owner_of(deps, env, token_id, include_expired.unwrap_or(false))?)
            }
            QueryMsg::AllNftInfo {
                token_id,
                include_expired,
            } => to_binary(&self.all_nft_info(
                deps,
                env,
                token_id,
                include_expired.unwrap_or(false),
            )?),
            QueryMsg::Operator {
                owner,
                operator,
                include_expired,
            } => to_binary(&self.operator(
                deps,
                env,
                owner,
                operator,
                include_expired.unwrap_or(false),
            )?),
            QueryMsg::AllOperators {
                owner,
                include_expired,
                start_after,
                limit,
            } => to_binary(&self.operators(
                deps,
                env,
                owner,
                include_expired.unwrap_or(false),
                start_after,
                limit,
            )?),
            QueryMsg::NumTokens {} => to_binary(&self.num_tokens(deps)?),
            QueryMsg::Tokens {
                owner,
                start_after,
                limit,
            } => to_binary(&self.tokens(deps, owner, start_after, limit)?),
            QueryMsg::AllTokens { start_after, limit } => {
                to_binary(&self.all_tokens(deps, start_after, limit)?)
            }
            QueryMsg::Approval {
                token_id,
                spender,
                include_expired,
            } => to_binary(&self.approval(
                deps,
                env,
                token_id,
                spender,
                include_expired.unwrap_or(false),
            )?),
            QueryMsg::Approvals {
                token_id,
                include_expired,
            } => {
                to_binary(&self.approvals(deps, env, token_id, include_expired.unwrap_or(false))?)
            }
            QueryMsg::Ownership {} => to_binary(&Self::ownership(deps)?),
            QueryMsg::Extension { msg: _ } => Ok(Binary::default()),
        }
    }

    pub fn minter(&self, deps: Deps) -> StdResult<MinterResponse> {
        let minter = Some(self.minter.load(deps.storage)?.to_string());

        Ok(MinterResponse { minter })
    }

    pub fn ownership(deps: Deps) -> StdResult<cw_ownable::Ownership<Addr>> {
        cw_ownable::get_ownership(deps.storage)
    }
}

fn parse_approval(item: StdResult<(Addr, Expiration)>) -> StdResult<cw721::Approval> {
    item.map(|(spender, expires)| cw721::Approval {
        spender: spender.to_string(),
        expires,
    })
}

fn humanize_approvals<T>(
    block: &BlockInfo,
    info: &TokenInfo<T>,
    include_expired: bool,
) -> Vec<cw721::Approval> {
    info.approvals
        .iter()
        .filter(|apr| include_expired || !apr.is_expired(block))
        .map(humanize_approval)
        .collect()
}

fn humanize_approval(approval: &Approval) -> cw721::Approval {
    cw721::Approval {
        spender: approval.spender.to_string(),
        expires: approval.expires,
    }
}
