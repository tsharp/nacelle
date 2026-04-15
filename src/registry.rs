use crate::error::NacelleError;
use crate::handler::BoxedHandler;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryStrategy {
    Dense,
    Sparse,
}

enum RegistryStorage<Svc, Req> {
    Dense {
        base: u64,
        handlers: Box<[Option<BoxedHandler<Svc, Req>>]>,
    },
    Sparse {
        handlers: Box<[(u64, BoxedHandler<Svc, Req>)]>,
    },
}

pub struct HandlerRegistry<Svc, Req> {
    storage: RegistryStorage<Svc, Req>,
    default: Option<BoxedHandler<Svc, Req>>,
    strategy: RegistryStrategy,
}

impl<Svc, Req> HandlerRegistry<Svc, Req> {
    pub fn build(
        mut handlers: Vec<(u64, BoxedHandler<Svc, Req>)>,
        default: Option<BoxedHandler<Svc, Req>>,
    ) -> Result<Self, NacelleError> {
        if handlers.is_empty() && default.is_none() {
            return Err(NacelleError::MissingHandler);
        }

        handlers.sort_unstable_by_key(|(opcode, _)| *opcode);
        for window in handlers.windows(2) {
            if window[0].0 == window[1].0 {
                return Err(NacelleError::DuplicateHandler(window[0].0));
            }
        }

        let strategy = select_strategy(&handlers);
        let storage = match strategy {
            RegistryStrategy::Dense => {
                let base = handlers.first().map(|(opcode, _)| *opcode).unwrap_or(0);
                let max = handlers.last().map(|(opcode, _)| *opcode).unwrap_or(base);
                let len = (max - base + 1) as usize;
                let mut dense = vec![None; len];
                for (opcode, handler) in handlers {
                    dense[(opcode - base) as usize] = Some(handler);
                }

                RegistryStorage::Dense {
                    base,
                    handlers: dense.into_boxed_slice(),
                }
            }
            RegistryStrategy::Sparse => RegistryStorage::Sparse {
                handlers: handlers.into_boxed_slice(),
            },
        };

        Ok(Self {
            storage,
            default,
            strategy,
        })
    }

    pub fn strategy(&self) -> RegistryStrategy {
        self.strategy
    }

    pub fn get(&self, opcode: u64) -> Option<&BoxedHandler<Svc, Req>> {
        match &self.storage {
            RegistryStorage::Dense { base, handlers } => {
                let offset = opcode.checked_sub(*base)? as usize;
                handlers.get(offset)?.as_ref()
            }
            RegistryStorage::Sparse { handlers } => handlers
                .binary_search_by_key(&opcode, |(value, _)| *value)
                .ok()
                .map(|index| &handlers[index].1),
        }
    }

    pub fn resolve(&self, opcode: u64) -> Option<&BoxedHandler<Svc, Req>> {
        self.get(opcode).or(self.default.as_ref())
    }
}

fn select_strategy<Svc, Req>(handlers: &[(u64, BoxedHandler<Svc, Req>)]) -> RegistryStrategy {
    let Some((min, _)) = handlers.first() else {
        return RegistryStrategy::Sparse;
    };
    let Some((max, _)) = handlers.last() else {
        return RegistryStrategy::Sparse;
    };

    let span = (*max - *min + 1) as usize;
    if span <= 4096 && span <= handlers.len().saturating_mul(4) {
        RegistryStrategy::Dense
    } else {
        RegistryStrategy::Sparse
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::handler::handler_fn;
    use crate::request::{RequestBody, ResponseWriter};

    use super::*;

    #[test]
    fn prefers_dense_when_opcodes_are_compact() {
        let handler = handler_fn(
            |_svc: Arc<()>, _req: (), _body: RequestBody, _response: ResponseWriter| async move {
                Ok(())
            },
        );
        let registry = HandlerRegistry::build(
            vec![(1, handler.clone()), (2, handler.clone()), (3, handler)],
            None,
        )
        .expect("registry should build");

        assert_eq!(registry.strategy(), RegistryStrategy::Dense);
        assert!(registry.get(2).is_some());
    }

    #[test]
    fn falls_back_to_sparse_for_wide_ranges() {
        let handler = handler_fn(
            |_svc: Arc<()>, _req: (), _body: RequestBody, _response: ResponseWriter| async move {
                Ok(())
            },
        );
        let registry = HandlerRegistry::build(vec![(1, handler.clone()), (8192, handler)], None)
            .expect("registry should build");

        assert_eq!(registry.strategy(), RegistryStrategy::Sparse);
        assert!(registry.get(8192).is_some());
    }
}
