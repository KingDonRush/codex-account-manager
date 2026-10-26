use crate::domain::deletion::{DeletionRequest, DeletionState};
use crate::error::AppError;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};

#[derive(Default)]
pub struct DeletionBook(Mutex<Book>);
#[derive(Default)]
struct Book {
    requests: HashMap<String, DeletionRequest>,
    order: VecDeque<String>,
}
impl DeletionBook {
    pub fn add(&self, request: DeletionRequest) -> Result<(), AppError> {
        let mut book = self.0.lock();
        if book
            .requests
            .values()
            .filter(|request| !request.state.terminal())
            .count()
            >= 8
        {
            return Err(AppError::Busy(
                "Há pedidos de exclusão pendentes.".to_owned(),
            ));
        }
        // Keep recent receipts without ever evicting a confirmation in progress.
        if book.requests.len() >= 64 {
            if let Some(position) = book.order.iter().position(|id| {
                book.requests
                    .get(id)
                    .is_some_and(|request| request.state.terminal())
            }) {
                if let Some(id) = book.order.remove(position) {
                    book.requests.remove(&id);
                }
            }
        }
        if book.requests.contains_key(&request.id) {
            return Err(AppError::Busy("Pedido já registrado.".to_owned()));
        }
        book.order.push_back(request.id.clone());
        book.requests.insert(request.id.clone(), request);
        Ok(())
    }
    pub fn pending(&self) -> Vec<DeletionRequest> {
        let book = self.0.lock();
        book.order
            .iter()
            .filter_map(|id| book.requests.get(id))
            .filter(|request| request.state == DeletionState::AwaitingConfirmation)
            .cloned()
            .collect()
    }
    pub fn get(&self, id: &str) -> Result<DeletionRequest, AppError> {
        self.0
            .lock()
            .requests
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::OperationNotFound(id.to_owned()))
    }
    pub fn begin(&self, id: &str) -> Result<DeletionRequest, AppError> {
        let mut book = self.0.lock();
        let request = book
            .requests
            .get_mut(id)
            .ok_or_else(|| AppError::OperationNotFound(id.to_owned()))?;
        if request.state != DeletionState::AwaitingConfirmation {
            return Err(AppError::OperationFinished);
        }
        request.state = DeletionState::Confirming;
        Ok(request.clone())
    }
    pub fn save(&self, request: DeletionRequest) {
        if let Some(current) = self.0.lock().requests.get_mut(&request.id) {
            *current = request;
        }
    }
    pub fn deny(&self, id: &str) -> Result<(), AppError> {
        let mut book = self.0.lock();
        let request = book
            .requests
            .get_mut(id)
            .ok_or_else(|| AppError::OperationNotFound(id.to_owned()))?;
        if request.state != DeletionState::AwaitingConfirmation {
            return Err(AppError::OperationFinished);
        }
        request.state = DeletionState::Denied;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
