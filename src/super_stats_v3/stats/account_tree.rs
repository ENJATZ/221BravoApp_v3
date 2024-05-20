use std::ops::Add;

use candid::{CandidType, Nat};
use ic_stable_memory::{derive::{StableType, AsFixedSizeBytes}, collections::{SBTreeMap, SVec}};
use num_bigint::BigUint;
use serde::{Deserialize, Serialize};

use crate::core::{runtime::RUNTIME_STATE, utils::log, stable_memory::Main, types::IDKey};

use super::custom_types::{SmallTX, ProcessedTX};

// Stable Store of account indexed data
#[derive(StableType, AsFixedSizeBytes, Debug, Default)]
pub struct AccountTree{
    pub accounts: SBTreeMap<u64, Overview>,
    pub accounts_history: SBTreeMap<(u64, u64), HistoryData>,
    count: u64, // not used
    last_updated: u64, // not used
}

impl AccountTree {

    fn update_history_balance(&mut self, account_ref: &u64, stx: &SmallTX, tx_type: TransactionType) {
        let day_of_transaction = stx.time / (86400 * 1_000_000_000);
        let account_history_key = (*account_ref, day_of_transaction);

        let fee: u128;
        if let Some(f) = stx.fee { fee = f } else {
            fee = RUNTIME_STATE.with(|s|{s.borrow().data.get_ledger_fee()})
        };
        
        if !self.accounts_history.contains_key(&account_history_key) && tx_type == TransactionType::In {
            let previous_day_balance = self.accounts_history.get(&(*account_ref, day_of_transaction - 1)).map_or(0, |previous_day| previous_day.balance);

            let new_balance = match tx_type {
                TransactionType::In => previous_day_balance.saturating_add(stx.value),
                TransactionType::Out => previous_day_balance.saturating_sub(stx.value).saturating_sub(fee)
            };

            self.accounts_history.insert(account_history_key, HistoryData {
                balance: new_balance,
            }).expect("Storage is full");
            
        }
        else if let Some(mut ach) = self.accounts_history.get_mut(&account_history_key) {
            ach.balance = match tx_type {
                TransactionType::In => ach.balance.saturating_add(stx.value),
                TransactionType::Out => ach.balance.saturating_sub(stx.value).saturating_sub(fee)
            };
            // Should we update further days balance here as well?
            // Will we have a case where we process a new transaction before an old transaction? 
        }
    }

    pub fn process_transfer_to(&mut self, account_ref: &u64, stx: &SmallTX) -> Result<String, String> {
      self.update_history_balance(account_ref, stx, TransactionType::In);

      if !self.accounts.contains_key(account_ref) {
         let acd = Overview { 
                  first_active: stx.time, 
                  last_active: stx.time, 
                  sent: (0_u32, 0_u128), 
                  received: (1_u32, stx.value), 
                  balance: stx.value 
               };

         self.accounts.insert(*account_ref, acd).expect("Storage is full");
         return Ok("Transfer Processed".to_string())

      } else if let Some(mut ac) = self.accounts.get_mut(account_ref) {
         ac.credit_account(stx.time, stx.value);
        
         return Ok("Transfer Processed".to_string())
      } else {
        let error = 
        format!("Error - cannot process transfer to, Block: {}", stx);
        return Err(error);
      }
    }

    pub fn process_transfer_from(&mut self, account_ref: &u64, stx: &SmallTX ) -> Result<String, String> {
        self.update_history_balance(account_ref, stx, TransactionType::Out);

        match self.accounts.get_mut(account_ref) {
            Some(mut ac) => {
                let fee: u128;
                if let Some(f) = stx.fee { fee = f } else {
                  fee = RUNTIME_STATE.with(|s|{s.borrow().data.get_ledger_fee()})
                };
                ac.debit_account(stx.time, stx.value, fee);
                return Ok("Processed OK".to_string());
            },
            None => { 
               let error = 
               format!("Error - cannot send from a non-existent account (process_transfer_from), Block: {}", stx);
               return Err(error);
            },
        }
    }

    pub fn process_approve_from(&mut self, account_ref: &u64, stx: &SmallTX ) -> Result<String, String> {
        let stx_zero_value = SmallTX {
            value: 0,
            ..*stx
        };
        self.update_history_balance(account_ref, &stx_zero_value, TransactionType::Out);

        match self.accounts.get_mut(account_ref) {
            Some(mut ac) => {
               let fee: u128;
               if let Some(f) = stx.fee { fee = f } else {
                 fee = RUNTIME_STATE.with(|s|{s.borrow().data.get_ledger_fee()})
               };
                ac.debit_account(stx.time, 0, fee);
                return Ok("Approve Processed".to_string());
            },
            None => { return Err("Error - cannot send from a non-existent account (process_approve_from)".to_string())},
        }
    }
}

#[derive(CandidType, StableType, Deserialize, Serialize, Clone, Default, AsFixedSizeBytes, Debug)]
pub struct HistoryData {
    pub balance: u128,
}
impl Add for HistoryData {
    type Output = HistoryData;

    fn add(self, other: Self) -> Self::Output {
        HistoryData {
            balance: self.balance + other.balance,
        }
    }
}
// TODO: Move out of here
#[derive(CandidType, StableType, Deserialize, Serialize, Clone, Default, Debug)]
pub struct GetAccountBalanceHistory {
    pub account: String,
    pub days: u64,
    pub merge_subaccounts: bool,
}
#[derive(PartialEq)]
enum TransactionType {
    In,
    Out
}
#[derive(StableType, AsFixedSizeBytes, Debug, Default)]
pub struct AccountData {
    pub overview: Overview
 }

 #[derive(CandidType, StableType, Deserialize, Serialize, Clone, Default, AsFixedSizeBytes, Debug)]
 pub struct Overview {
    pub first_active: u64,
    pub last_active: u64,
    pub sent: (u32, u128), // count, value
    pub received: (u32, u128), // count, value
    pub balance: u128,
 }
 impl Overview {
    pub fn debit_account(&mut self, time:u64, value: u128, tx_fee: u128){
        if self.first_active == 0 || time < self.first_active { self.first_active = time }
        if self.last_active < time { self.last_active = time }

        // update balances
        let total_deduction = value.saturating_add(tx_fee);
        self.balance = self.balance.saturating_sub(total_deduction);
        let (mut s1, mut s2) = self.sent;
        s1 += 1;
        s2 = s2.saturating_add(total_deduction);
        self.sent = (s1,s2);
    }

    pub fn credit_account(&mut self, time:u64, value: u128){
        if self.first_active == 0 || time < self.first_active { self.first_active = time }
        if self.last_active < time {self.last_active = time}

        // update balances
        self.balance = self.balance.saturating_add(value);
        let (mut r1, mut r2) = self.received;
        r1 += 1;
        r2 = r2.saturating_add(value);
        self.received = (r1,r2);
    }
 }

 // IMPL 
 impl Main {
    pub fn get_overview_by_id(&self, id_string: &String) -> Option<Overview> {
        match self.directory_data.get_ref(id_string) {
            Some(ref_value) => {
                match self.account_data.accounts.get(&ref_value) {
                    Some(ac_value) => { 
                        let ov = Overview{
                            first_active: ac_value.first_active,
                            last_active: ac_value.last_active,
                            sent: ac_value.sent,
                            received: ac_value.received,
                            balance: ac_value.balance,
                        };
                        return Some(ov);
                    },
                    None => {return None}
                }
            },
            None => { return None },
        } 
    }

    pub fn get_overview_by_ref(&self, id_ref: &u64) -> Option<Overview> {
        match self.account_data.accounts.get(&id_ref) {
            Some(ac_value) => { 
                let ov = Overview{
                    first_active: ac_value.first_active,
                    last_active: ac_value.last_active,
                    sent: ac_value.sent,
                    received: ac_value.received,
                    balance: ac_value.balance,
                };
                return Some(ov);
            },
            None => {return None}
        }
    }
}