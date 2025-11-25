//! address book for wallet contacts
//! stores contacts encrypted alongside wallet data

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// a contact in the address book
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    pub id: String,
    pub name: String,
    pub address: String,
    pub notes: Option<String>,
    pub last_message_time: Option<u64>,
    pub unread_count: u32,
}

impl Contact {
    pub fn new(name: &str, address: &str) -> Self {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(address.as_bytes());
        hasher.update(name.as_bytes());
        let id = hex::encode(&hasher.finalize()[..8]);

        Self {
            id,
            name: name.to_string(),
            address: address.to_string(),
            notes: None,
            last_message_time: None,
            unread_count: 0,
        }
    }

    /// generate avatar color from address hash
    pub fn avatar_color(&self) -> [u8; 3] {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(self.address.as_bytes());
        let hash = hasher.finalize();
        [hash[0], hash[1], hash[2]]
    }

    /// first letter of name for avatar
    pub fn avatar_letter(&self) -> char {
        self.name.chars().next().unwrap_or('?').to_ascii_uppercase()
    }
}

/// address book storage
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AddressBook {
    contacts: HashMap<String, Contact>,
}

impl AddressBook {
    pub fn new() -> Self {
        Self {
            contacts: HashMap::new(),
        }
    }

    /// add a new contact
    pub fn add(&mut self, name: &str, address: &str) -> Result<Contact> {
        let contact = Contact::new(name, address);
        self.contacts.insert(contact.id.clone(), contact.clone());
        Ok(contact)
    }

    /// remove a contact by id
    pub fn remove(&mut self, id: &str) -> Option<Contact> {
        self.contacts.remove(id)
    }

    /// get contact by id
    pub fn get(&self, id: &str) -> Option<&Contact> {
        self.contacts.get(id)
    }

    /// get mutable contact by id
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Contact> {
        self.contacts.get_mut(id)
    }

    /// find contact by address
    pub fn find_by_address(&self, address: &str) -> Option<&Contact> {
        self.contacts.values().find(|c| c.address == address)
    }

    /// list all contacts sorted by name
    pub fn list(&self) -> Vec<&Contact> {
        let mut contacts: Vec<_> = self.contacts.values().collect();
        contacts.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        contacts
    }

    /// list contacts sorted by recent messages
    pub fn list_by_recent(&self) -> Vec<&Contact> {
        let mut contacts: Vec<_> = self.contacts.values().collect();
        contacts.sort_by(|a, b| {
            match (b.last_message_time, a.last_message_time) {
                (Some(b_time), Some(a_time)) => b_time.cmp(&a_time),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });
        contacts
    }

    /// update contact name
    pub fn rename(&mut self, id: &str, new_name: &str) -> Result<()> {
        if let Some(contact) = self.contacts.get_mut(id) {
            contact.name = new_name.to_string();
        }
        Ok(())
    }

    /// update last message time for a contact
    pub fn update_message_time(&mut self, id: &str, time: u64) {
        if let Some(contact) = self.contacts.get_mut(id) {
            contact.last_message_time = Some(time);
        }
    }

    /// increment unread count
    pub fn increment_unread(&mut self, id: &str) {
        if let Some(contact) = self.contacts.get_mut(id) {
            contact.unread_count += 1;
        }
    }

    /// clear unread count
    pub fn clear_unread(&mut self, id: &str) {
        if let Some(contact) = self.contacts.get_mut(id) {
            contact.unread_count = 0;
        }
    }

    /// total unread messages
    pub fn total_unread(&self) -> u32 {
        self.contacts.values().map(|c| c.unread_count).sum()
    }

    /// count contacts
    pub fn len(&self) -> usize {
        self.contacts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.contacts.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_contact() {
        let mut book = AddressBook::new();
        let contact = book.add("alice", "zs1alice...").unwrap();
        assert_eq!(contact.name, "alice");
        assert_eq!(book.len(), 1);
    }

    #[test]
    fn test_find_by_address() {
        let mut book = AddressBook::new();
        book.add("bob", "zs1bob...").unwrap();
        let found = book.find_by_address("zs1bob...");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "bob");
    }
}
