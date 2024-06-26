use near_sdk::collections::{LookupMap, UnorderedSet};
use near_sdk::json_types::Base64VecU8;
use near_sdk::{
    ext_contract, log, Allowance, NearToken, PanicOnDefault, Promise, PromiseResult
};
use near_sdk::{env, near, AccountId, PublicKey};
use near_sdk::{serde_json, Gas};
use std::convert::TryFrom;
use std::str::FromStr;

// 5 Ⓝ in yoctoNEAR
const PRIZE_AMOUNT: u128 = 5_000_000_000_000_000_000_000_000;
// TODO: tune these
const GAS_FOR_ACCOUNT_CREATION: Gas = Gas::from_gas(150_000_000_000_000);
const GAS_FOR_ACCOUNT_CALLBACK: Gas = Gas::from_gas(110_000_000_000_000);

/// Used to call the linkdrop contract deployed to the top-level account
///   (like "testnet")
#[ext_contract(ext_linkdrop)]
pub trait ExtLinkDropCrossContract {
    fn create_account(&mut self, new_account_id: AccountId, new_public_key: PublicKey) -> Promise;
}

/// Define the callbacks in this smart contract:
///   1. See how the Transfer Action went when the user has an account
///   2. See how the "create_account" went when the user wishes to create an account
///      (Returns true if the account was created successfully
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

//#[derive(BorshDeserialize, BorshSerialize, Deserialize, Serialize, Debug)]
//#[serde(crate = "near_sdk::serde")]
#[near(serializers = [borsh, json])]
pub enum AnswerDirection {
    Across,
    Down,
}

/// The origin (0,0) starts at the top left side of the square
//#[derive(BorshDeserialize, BorshSerialize, Deserialize, Serialize, Debug)]
//#[serde(crate = "near_sdk::serde")]
#[near(serializers = [borsh, json])]
pub struct CoordinatePair {
    x: u8,
    y: u8,
}

// {"num": 1, "start": {"x": 19, "y": 31}, "direction": "Across", "length": 8, "clue": "not far but"}
// We'll have the clue stored on-chain for now for simplicity.
//#[derive(BorshDeserialize, BorshSerialize, Deserialize, Serialize, Debug)]
//#[serde(crate = "near_sdk::serde")]
#[near(serializers = [borsh, json])]
pub struct Answer {
    num: u8,
    start: CoordinatePair,
    direction: AnswerDirection,
    length: u8,
    clue: String,
}

//#[derive(BorshDeserialize, BorshSerialize, Deserialize, Serialize, Debug)]
//#[serde(crate = "near_sdk::serde")]
#[near(serializers = [borsh, json])]
pub enum PuzzleStatus {
    Unsolved,
    Solved { solver_pk: String },
    Claimed { memo: String },
}

//#[derive(Serialize)]
//#[serde(crate = "near_sdk::serde")]
#[near(serializers = [borsh, json])]
pub struct UnsolvedPuzzles {
    puzzles: Vec<JsonPuzzle>,
    creator_account: AccountId,
}

//#[derive(Serialize, Deserialize)]
//#[serde(crate = "near_sdk::serde")]
#[near(serializers = [borsh, json])]
pub struct JsonPuzzle {
    /// The human-readable public key that's the solution from the seed phrase
    solution_public_key: String,
    status: PuzzleStatus,
    reward: NearToken,
    creator: AccountId,
    dimensions: CoordinatePair,
    answer: Vec<Answer>,
}

//#[derive(BorshDeserialize, BorshSerialize, Debug)]
#[near(serializers = [borsh])]
pub struct Puzzle {
    status: PuzzleStatus,
    reward: NearToken,
    creator: AccountId,
    /// Use the CoordinatePair assuming the origin is (0, 0) in the top left side of the puzzle.
    dimensions: CoordinatePair,
    answer: Vec<Answer>,
}

//#[derive(Serialize, Deserialize, BorshDeserialize, BorshSerialize, Debug)]
//#[serde(crate = "near_sdk::serde")]
#[near(serializers = [borsh, json])]
pub struct NewPuzzleArgs {
    answer_pk: String,
    dimensions: CoordinatePair,
    answers: Vec<Answer>,
}

/// Regarding PanicOnDefault:
/// When you want to have a "new" function initialize a smart contract,
/// you'll likely want to follow this pattern of having a default implementation that panics,
/// directing the user to call the initialization method. (The one with the #[init] macro)
//#[near_bindgen]
//#[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Crossword {
    owner_id: AccountId,
    puzzles: LookupMap<String, Puzzle>,
    unsolved_puzzles: UnorderedSet<String>,
    /// When a user solves the puzzle and goes to claim the reward, they might need to create an account. This is the account that likely contains the "linkdrop" smart contract. https://github.com/near/near-linkdrop
    creator_account: AccountId,
}

#[near]
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
            .get(&String::from(&answer_pk))
            .expect("ERR_NOT_CORRECT_ANSWER");

        // Check if the puzzle is already solved. If it's unsolved, make batch action of
        // removing that public key and adding the user's public key
        puzzle.status = match puzzle.status {
            PuzzleStatus::Unsolved => PuzzleStatus::Solved {
                solver_pk: String::from(&solver_pk),
            },
            _ => {
                env::panic_str("ERR_PUZZLE_SOLVED");
            }
        };

        // Reinsert the puzzle back in after we modified the status:
        self.puzzles.insert(&String::from(&answer_pk), &puzzle);
        // Remove from the list of unsolved ones
        self.unsolved_puzzles.remove(&String::from(&answer_pk));

        log!(
            "Puzzle with pk {:?} solved, solver pk: {}",
            answer_pk,
            String::from(&solver_pk)
        );

        // Add new function call access key able to call claim_reward and claim_reward_new_account
        Promise::new(env::current_account_id()).add_access_key_allowance(
            solver_pk.into(),
            Allowance::limited(NearToken::from_yoctonear(250000000000000000000000)).unwrap(),
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
            .get(&String::from(&crossword_pk))
            .expect("That puzzle doesn't exist");

        // Check that puzzle is solved and the signer has the right public key
        match puzzle.status {
            PuzzleStatus::Solved {
                solver_pk: puzzle_pk,
            } => {
                // Check to see if signer_pk matches
                assert_eq!(String::from(&signer_pk), puzzle_pk, "You're not the person who can claim this, or else you need to use your function-call access key, friend.");
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

        ext_linkdrop::ext(AccountId::from(self.creator_account.clone()))
            .with_attached_deposit(reward_amount)
            .with_static_gas(GAS_FOR_ACCOUNT_CREATION) // This amount of gas will be split
            .create_account(new_acc_id.parse().unwrap(), new_pk)
            .then(
                // Chain a promise callback to ourselves
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_ACCOUNT_CALLBACK)
                    .callback_after_create_account(
                        crossword_pk,
                        new_acc_id,
                        memo,
                        env::signer_account_pk(),
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
            .get(&String::from(&crossword_pk))
            .expect("That puzzle doesn't exist");

        // Check that puzzle is solved and the signer has the right public key
        match puzzle.status {
            PuzzleStatus::Solved {
                solver_pk: puzzle_pk,
            } => {
                // Check to see if signer_pk matches
                assert_eq!(String::from(&signer_pk), puzzle_pk, "You're not the person who can claim this, or else you need to use your function-call access key, friend.");
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
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_ACCOUNT_CALLBACK)
                    .callback_after_transfer(
                        crossword_pk,
                        receiver_acc_id,
                        memo,
                        env::signer_account_pk(),
                    ),
            )
    }

    /// Puzzle creator provides:
    /// `answer_pk` - a public key generated from crossword answer (seed phrase)
    /// `dimensions` - the shape of the puzzle, lengthwise (`x`) and high (`y`) (Soon to be deprecated)
    /// `answers` - the answers for this puzzle
    /// Call with NEAR CLI like so:
    /// `near call $NEAR_ACCT new_puzzle '{"answer_pk": "ed25519:psA2GvARwAbsAZXPs6c6mLLZppK1j1YcspGY2gqq72a", "dimensions": {"x": 19, "y": 13}, "answers": [{"num": 1, "start": {"x": 19, "y": 31}, "direction": "Across", "length": 8}]}' --accountId $NEAR_ACCT`
    pub fn new_puzzle(&mut self, args: Base64VecU8) {
        assert_eq!(
            env::predecessor_account_id(),
            self.owner_id,
            "Only the owner may call this method"
        );
        // We'll turn the base64 vector of bytes into our argument object
        let puzzle_args: NewPuzzleArgs = serde_json::from_slice(&args.0.as_slice()).unwrap();

        let creator = env::predecessor_account_id();
        let answer_pk = PublicKey::from_str(puzzle_args.answer_pk.as_str()).unwrap();
        let existing = self.puzzles.insert(
            &puzzle_args.answer_pk,
            &Puzzle {
                status: PuzzleStatus::Unsolved,
                reward: NearToken::from_yoctonear(PRIZE_AMOUNT),
                creator,
                dimensions: puzzle_args.dimensions,
                answer: puzzle_args.answers,
            },
        );

        assert!(existing.is_none(), "Puzzle with that key already exists");
        self.unsolved_puzzles.insert(&puzzle_args.answer_pk);

        Promise::new(env::current_account_id()).add_access_key_allowance(
            answer_pk,
            Allowance::limited(NearToken::from_yoctonear(250000000000000000000000)).unwrap(),
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
                solution_public_key: get_decoded_pk(PublicKey::from_str(pk.as_str()).unwrap()),
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
#[near]
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
            .get(&String::from(&crossword_pk))
            .expect("Error loading puzzle when finalizing.");

        puzzle.status = PuzzleStatus::Claimed { memo: memo.clone() };
        // Reinsert the puzzle back in after we modified the status
        self.puzzles.insert(&String::from(&crossword_pk), &puzzle);

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

#[near]
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
