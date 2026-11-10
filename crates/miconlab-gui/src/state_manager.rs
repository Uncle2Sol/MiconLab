// 统一面板状态管理器
//
// 封装 egui ctx.data_mut() 的 clone-before/write-back 模式，
// 解决 CentralPanel 闭包内 ctx.data_mut() 死锁问题。
//
// egui 0.29 的 CentralPanel::show 持有 Context 内部 RwLock 的写锁，
// 闭包内再调用 ctx.data_mut() 会重入写锁导致永久死锁。
// 因此在 CentralPanel 之前取出持久化状态的克隆，闭包内使用克隆，
// 闭包结束后再写回。
//
// 使用方式：
// 1. 在 CentralPanel 之前调用 load_state() 获取状态克隆
// 2. 在 CentralPanel 闭包内使用克隆的状态
// 3. 在 CentralPanel 之后调用 save_state() 写回状态

use egui::{Context, Id};

/// 在 CentralPanel 之前加载面板状态（克隆取出）
///
/// 从 egui 的临时数据存储中获取指定面板的状态克隆。
/// 必须在 CentralPanel::show() 之前调用，避免死锁。
pub fn load_state<T: Default + Clone + Send + Sync + 'static>(ctx: &Context, id: Id) -> T {
    ctx.data_mut(|data| data.get_temp_mut_or_default::<T>(id).clone())
}

/// 在 CentralPanel 之后保存面板状态（写回）
///
/// 将修改后的状态写回 egui 的临时数据存储。
/// 必须在 CentralPanel::show() 之后调用，避免死锁。
pub fn save_state<T: Default + Clone + Send + Sync + 'static>(ctx: &Context, id: Id, state: T) {
    ctx.data_mut(|data| {
        *data.get_temp_mut_or_default::<T>(id) = state;
    });
}
