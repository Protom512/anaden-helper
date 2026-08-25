//! ミニゲーム戦略の実装とレジストリ。

mod catalog;
mod registry;

pub use catalog::{
    SelectionError, StrategyCatalog, StrategyDef, StrategyOptionDef, StrategySelection,
};
pub use registry::StrategyRegistry;
