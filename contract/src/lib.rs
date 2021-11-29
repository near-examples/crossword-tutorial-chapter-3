use std::convert::TryFrom;
use near_sdk::collections::{LookupMap, UnorderedSet};
use near_sdk::{Gas, serde_json};
use near_sdk::{
    borsh::{self, BorshDeserialize, BorshSerialize},
    ext_contract, log,
    serde::{Deserialize, Serialize},
    Balance, Promise, PromiseResult, PanicOnDefault
};
use near_sdk::{env, near_bindgen, PublicKey, AccountId};

// 5 Ⓝ in yoctoNEAR
const PRIZE_AMOUNT: u128 = 5_000_000_000_000_000_000_000_000;
// TODO: tune these
const GAS_FOR_ACCOUNT_CREATION: Gas = Gas(150_000_000_000_000);
const GAS_FOR_ACCOUNT_CALLBACK: Gas = Gas(110_000_000_000_000);

/// Used to call the linkdrop contract deployed to the top-level account
///   (like "testnet")
#[ext_contract(ext_linkdrop)]
pub trait ExtLinkDropCrossContract {
    fn create_account(
        &mut self,
        new_account_id: AccountId,
        new_public_key: PublicKey,
    ) -> Promise;
}

/// Define the callbacks in this smart contract:
///   1. See how the Transfer Action went when the user has an account
///   2. See how the "create_account" went when the user wishes to create an account
///      (Returns true if the account was created successfully)
#[ext_contract(ext_self)]
pub trait AfterClaim {
    fn callback_after_transfer(
        &mut self,
        crossword_pk: PublicKey,
        account_id: String,
        memo: String,
        signer_pk: PublicKey,
    ) -> bool;
    fn callback_after_create_account(
        &mut self,
        crossword_pk: PublicKey,
        account_id: String,
        memo: String,
        signer_pk: PublicKey,
    ) -> bool;
}

/// Unfortunately, you have to double this trait, once for the cross-contract call,
///   and once so Rust knows about it and we can implement this callback.
pub trait AfterClaim {
    fn callback_after_transfer(
        &mut self,
        crossword_pk: PublicKey,
        account_id: String,
        memo: String,
        signer_pk: PublicKey,
    ) -> bool;
    fn callback_after_create_account(
        &mut self,
        crossword_pk: PublicKey,
        account_id: String,
        memo: String,
        signer_pk: PublicKey,
    ) -> bool;
}

#[derive(BorshDeserialize, BorshSerialize, Deserialize, Serialize, Debug)]
#[serde(crate = "near_sdk::serde")]
pub enum AnswerDirection {
    Across,
    Down,
}

/// The origin (0,0) starts at the top left side of the square
#[derive(BorshDeserialize, BorshSerialize, Deserialize, Serialize, Debug)]
#[serde(crate = "near_sdk::serde")]
pub struct CoordinatePair {
    x: u8,
    y: u8,
}

// {"num": 1, "start": {"x": 19, "y": 31}, "direction": "Across", "length": 8, "clue": "not far but"}
// We'll have the clue stored on-chain for now for simplicity.
#[derive(BorshDeserialize, BorshSerialize, Deserialize, Serialize, Debug)]
#[serde(crate = "near_sdk::serde")]
pub struct Answer {
    num: u8,
    start: CoordinatePair,
    direction: AnswerDirection,
    length: u8,
    clue: String,
}

#[derive(BorshDeserialize, BorshSerialize, Deserialize, Serialize, Debug)]
#[serde(crate = "near_sdk::serde")]
pub enum PuzzleStatus {
    Unsolved,
    Solved { solver_pk: PublicKey },
    Claimed { memo: String },
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct UnsolvedPuzzles {
    puzzles: Vec<JsonPuzzle>,
    creator_account: AccountId,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct JsonPuzzle {
    /// The human-readable public key that's the solution from the seed phrase
    solution_public_key: String,
    status: PuzzleStatus,
    reward: Balance,
    creator: AccountId,
    dimensions: CoordinatePair,
    answer: Vec<Answer>,
}

#[derive(BorshDeserialize, BorshSerialize, Debug)]
pub struct Puzzle {
    status: PuzzleStatus,
    reward: Balance,
    creator: AccountId,
    /// Use the CoordinatePair assuming the origin is (0, 0) in the top left side of the puzzle.
    dimensions: CoordinatePair,
    answer: Vec<Answer>,
}

/// Regarding PanicOnDefault:
/// When you want to have a "new" function initialize a smart contract,
/// you'll likely want to follow this pattern of having a default implementation that panics,
/// directing the user to call the initialization method. (The one with the #[init] macro)
#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
pub struct Crossword {
    owner_id: AccountId,
    puzzles: LookupMap<PublicKey, Puzzle>,
    unsolved_puzzles: UnorderedSet<PublicKey>,
    /// When a user solves the puzzle and goes to claim the reward, they might need to create an account. This is the account that likely contains the "linkdrop" smart contract. https://github.com/near/near-linkdrop
    creator_account: AccountId,
}


#[near_bindgen]
impl Crossword {
    #[init]
    pub fn new(owner_id: AccountId, creator_account: AccountId) -> Self {
        Self {
            owner_id,
            puzzles: LookupMap::new(b"c"),
            unsolved_puzzles: UnorderedSet::new(b"u"),
            creator_account,
        }
    }

    pub fn submit_solution(&mut self, solver_pk: PublicKey) {
        let answer_pk = env::signer_account_pk();
        // check to see if the answer_pk from signer is in the puzzles
        let mut puzzle = self
            .puzzles
            .get(&answer_pk)
            .expect("ERR_NOT_CORRECT_ANSWER");

        // Check if the puzzle is already solved. If it's unsolved, make batch action of
        // removing that public key and adding the user's public key
        puzzle.status = match puzzle.status {
            PuzzleStatus::Unsolved => PuzzleStatus::Solved {
                solver_pk: solver_pk.clone().into(),
            },
            _ => {
                env::panic_str("ERR_PUZZLE_SOLVED");
            }
        };

        // Reinsert the puzzle back in after we modified the status:
        self.puzzles.insert(&answer_pk, &puzzle);
        // Remove from the list of unsolved ones
        self.unsolved_puzzles.remove(&answer_pk);

        log!(
            "Puzzle with pk {:?} solved, solver pk: {}",
            answer_pk,
            String::from(&solver_pk)
        );

        // Add new function call access key able to call claim_reward and claim_reward_new_account
        Promise::new(env::current_account_id()).add_access_key(
            solver_pk.into(),
            250000000000000000000000,
            env::current_account_id(),
            "claim_reward,claim_reward_new_account".to_string(),
        );

        // Delete old function call key
        Promise::new(env::current_account_id()).delete_key(answer_pk);
    }

    pub fn claim_reward_new_account(
        &mut self,
        crossword_pk: PublicKey,
        new_acc_id: String,
        new_pk: PublicKey,
        memo: String,
    ) -> Promise {
        let signer_pk = env::signer_account_pk();
        let puzzle = self
            .puzzles
            .get(&crossword_pk)
            .expect("That puzzle doesn't exist");

        // Check that puzzle is solved and the signer has the right public key
        match puzzle.status {
            PuzzleStatus::Solved {
                solver_pk: puzzle_pk,
            } => {
                // Check to see if signer_pk matches
                assert_eq!(signer_pk, puzzle_pk, "You're not the person who can claim this, or else you need to use your function-call access key, friend.");
            }
            _ => {
                env::panic_str("puzzle should have `Solved` status to be claimed");
            }
        };

        // Ensure there's enough balance to pay this out
        let reward_amount = puzzle.reward;
        assert!(
            env::account_balance() >= reward_amount,
            "The smart contract does not have enough balance to pay this out. :/"
        );

        ext_linkdrop::create_account(
            new_acc_id.parse().unwrap(),
            new_pk,
            AccountId::from(self.creator_account.clone()),
            reward_amount,
            GAS_FOR_ACCOUNT_CREATION,
        )
        .then(
            // Chain a promise callback to ourselves
            ext_self::callback_after_create_account(
                crossword_pk,
                new_acc_id,
                memo,
                env::signer_account_pk(),
                env::current_account_id(),
                0,
                GAS_FOR_ACCOUNT_CALLBACK,
            ),
        )
    }

    pub fn claim_reward(
        &mut self,
        crossword_pk: PublicKey,
        receiver_acc_id: String,
        memo: String,
    ) -> Promise {
        let signer_pk = env::signer_account_pk();
        // Check to see if the crossword_pk is in the puzzle's keys
        let puzzle = self
            .puzzles
            .get(&crossword_pk)
            .expect("That puzzle doesn't exist");

        // Check that puzzle is solved and the signer has the right public key
        match puzzle.status {
            PuzzleStatus::Solved {
                solver_pk: puzzle_pk,
            } => {
                // Check to see if signer_pk matches
                assert_eq!(signer_pk, puzzle_pk, "You're not the person who can claim this, or else you need to use your function-call access key, friend.");
            }
            _ => {
                env::panic_str("puzzle should have `Solved` status to be claimed");
            }
        };

        // Ensure there's enough balance to pay this out
        let reward_amount = puzzle.reward;
        assert!(
            env::account_balance() >= reward_amount,
            "The smart contract does not have enough balance to pay this out. :/"
        );

        Promise::new(receiver_acc_id.parse().unwrap())
            .transfer(reward_amount)
            .then(ext_self::callback_after_transfer(
                crossword_pk,
                receiver_acc_id,
                memo,
                env::signer_account_pk(),
                env::current_account_id(),
                0,
                GAS_FOR_ACCOUNT_CALLBACK,
            ))
    }

    /// Puzzle creator provides:
    /// `answer_pk` - a public key generated from crossword answer (seed phrase)
    /// `dimensions` - the shape of the puzzle, lengthwise (`x`) and high (`y`) (Soon to be deprecated)
    /// `answers` - the answers for this puzzle
    /// Call with NEAR CLI like so:
    /// `near call $NEAR_ACCT new_puzzle '{"answer_pk": "ed25519:psA2GvARwAbsAZXPs6c6mLLZppK1j1YcspGY2gqq72a", "dimensions": {"x": 19, "y": 13}, "answers": [{"num": 1, "start": {"x": 19, "y": 31}, "direction": "Across", "length": 8}]}' --accountId $NEAR_ACCT`
    pub fn new_puzzle(
        &mut self,
        answer_pk: PublicKey,
        dimensions: CoordinatePair,
        answers: Vec<Answer>,
    ) {
        assert_eq!(
            env::predecessor_account_id(),
            self.owner_id,
            "Only the owner may call this method"
        );
        let creator = env::predecessor_account_id();
        let answer_pk = PublicKey::from(answer_pk);
        let existing = self.puzzles.insert(
            &answer_pk,
            &Puzzle {
                status: PuzzleStatus::Unsolved,
                reward: PRIZE_AMOUNT,
                creator,
                dimensions,
                answer: answers,
            },
        );

        assert!(existing.is_none(), "Puzzle with that key already exists");
        self.unsolved_puzzles.insert(&answer_pk);

        Promise::new(env::current_account_id()).add_access_key(
            answer_pk,
            250000000000000000000000,
            env::current_account_id(),
            "submit_solution".to_string(),
        );
    }

    pub fn get_unsolved_puzzles(&self) -> UnsolvedPuzzles {
        let public_keys = self.unsolved_puzzles.to_vec();
        let mut all_unsolved_puzzles = vec![];
        for pk in public_keys {
            let puzzle = self
                .puzzles
                .get(&pk)
                .unwrap_or_else(|| env::panic_str("ERR_LOADING_PUZZLE"));
            let json_puzzle = JsonPuzzle {
                solution_public_key: get_decoded_pk(pk),
                status: puzzle.status,
                reward: puzzle.reward,
                creator: puzzle.creator,
                dimensions: puzzle.dimensions,
                answer: puzzle.answer,
            };
            all_unsolved_puzzles.push(json_puzzle)
        }
        UnsolvedPuzzles {
            puzzles: all_unsolved_puzzles,
            creator_account: self.creator_account.clone(),
        }
    }
}

/// Private functions (cannot be called from the outside by a transaction)
#[near_bindgen]
impl Crossword {
    /// Update the status of the puzzle and store the memo
    fn finalize_puzzle(
        &mut self,
        crossword_pk: PublicKey,
        account_id: String,
        memo: String,
        signer_pk: PublicKey,
    ) {
        let mut puzzle = self
            .puzzles
            .get(&crossword_pk)
            .expect("Error loading puzzle when finalizing.");

        puzzle.status = PuzzleStatus::Claimed { memo: memo.clone() };
        // Reinsert the puzzle back in after we modified the status
        self.puzzles.insert(&crossword_pk, &puzzle);

        log!(
            "Puzzle with pk: {:?} claimed, new account created: {}, memo: {}, reward claimed: {}",
            crossword_pk,
            account_id,
            memo,
            puzzle.reward
        );

        // Delete function-call access key
        Promise::new(env::current_account_id()).delete_key(signer_pk);
    }
}

#[near_bindgen]
impl AfterClaim for Crossword {
    #[private]
    fn callback_after_transfer(
        &mut self,
        crossword_pk: PublicKey,
        account_id: String,
        memo: String,
        signer_pk: PublicKey,
    ) -> bool {
        assert_eq!(
            env::promise_results_count(),
            1,
            "Expected 1 promise result."
        );
        match env::promise_result(0) {
            PromiseResult::NotReady => {
                unreachable!()
            }
            PromiseResult::Successful(_) => {
                // New account created and reward transferred successfully.
                self.finalize_puzzle(crossword_pk, account_id, memo, signer_pk);
                true
            }
            PromiseResult::Failed => {
                // Weren't able to create the new account,
                //   reward money has been returned to this contract.
                false
            }
        }
    }

    #[private]
    fn callback_after_create_account(
        &mut self,
        crossword_pk: PublicKey,
        account_id: String,
        memo: String,
        signer_pk: PublicKey,
    ) -> bool {
        assert_eq!(
            env::promise_results_count(),
            1,
            "Expected 1 promise result."
        );
        match env::promise_result(0) {
            PromiseResult::NotReady => {
                unreachable!()
            }
            PromiseResult::Successful(creation_result) => {
                let creation_succeeded: bool = serde_json::from_slice(&creation_result)
                    .expect("Could not turn result from account creation into boolean.");
                if creation_succeeded {
                    // New account created and reward transferred successfully.
                    self.finalize_puzzle(crossword_pk, account_id, memo, signer_pk);
                    true
                } else {
                    // Something went wrong trying to create the new account.
                    false
                }
            }
            PromiseResult::Failed => {
                // Problem with the creation transaction, reward money has been returned to this contract.
                false
            }
        }
    }
}

fn get_decoded_pk(pk: PublicKey) -> String {
    String::try_from(&pk).unwrap()
}

#[cfg(test)]
mod tests {
    // use super::*;
    // use near_sdk::MockedBlockchain;
    // use near_sdk::{testing_env, VMContext};

    // // mock the context for testing, notice "signer_account_id" that was accessed above from env::
    // fn get_context(input: Vec<u8>, is_view: bool) -> VMContext {
    //     VMContext {
    //         current_account_id: "alice_near".to_string(),
    //         signer_account_id: "bob_near".to_string(),
    //         signer_account_pk: vec![0, 1, 2],
    //         predecessor_account_id: "carol_near".to_string(),
    //         input,
    //         block_index: 0,
    //         block_timestamp: 0,
    //         account_balance: 0,
    //         account_locked_balance: 0,
    //         storage_usage: 0,
    //         attached_deposit: 0,
    //         prepaid_gas: 10u64.pow(18),
    //         random_seed: vec![0, 1, 2],
    //         is_view,
    //         output_data_receivers: vec![],
    //         epoch_height: 19,
    //     }
    // }
}
