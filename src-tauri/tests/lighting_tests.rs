use airpulse_lib::hid::{Air75Controller, LightingIo};
use airpulse_lib::lighting::{Air75MainState, Air75SideState};
use std::collections::VecDeque;

#[derive(Default)]
struct FakeIo {
    reads: VecDeque<anyhow::Result<[u8; 17]>>,
    writes: Vec<(u16, Vec<u8>, u8)>,
}

impl LightingIo for FakeIo {
    fn read_state(&mut self, _handle: u8) -> anyhow::Result<[u8; 17]> {
        self.reads.pop_front().expect("缺少模拟读取")
    }

    fn write(&mut self, address: u16, payload: &[u8], handle: u8) -> anyhow::Result<()> {
        self.writes.push((address, payload.to_vec(), handle));
        Ok(())
    }
}

fn state(main: Air75MainState, side: [u8; 8]) -> [u8; 17] {
    let mut state = [0_u8; 17];
    state[..9].copy_from_slice(main.as_bytes());
    state[9..].copy_from_slice(&side);
    state
}

fn side_state(main: Air75MainState, side: Air75SideState) -> [u8; 17] {
    let mut value = [0_u8; 17];
    value[..9].copy_from_slice(main.as_bytes());
    value[9..].copy_from_slice(side.as_bytes());
    value
}

#[test]
/// 主键区先以满亮度设置颜色，再独立提交目标亮度，之后不能重写颜色。
fn exercise_commits_each_main_state_then_brightness() {
    let side = [1, 2, 3, 4, 5, 6, 7, 8];
    let off = Air75MainState::off();
    let expected = Air75Controller::<FakeIo>::exercise_states();
    let mut io = FakeIo::default();
    io.reads.push_back(Ok(state(off, side)));
    for main in expected {
        io.reads.push_back(Ok(state(main, side)));
    }

    let mut controller = Air75Controller::new(io);
    controller.exercise(|_| {}).unwrap();
    let io = controller.into_inner();

    assert_eq!(io.writes.len(), 8);
    for (writes, main) in io.writes.chunks_exact(2).zip(expected) {
        let mut color = *main.as_bytes();
        color[1] = 100;
        assert_eq!(writes[0], (0, color.to_vec(), 0));
        assert_eq!(writes[1], (1, vec![main.as_bytes()[1]], 0));
    }
}

#[test]
/// 回读错误应保留原始参数，便于区分固件反馈与实体效果。
fn apply_rejects_main_readback_mismatch() {
    let side = [9; 8];
    let mut io = FakeIo::default();
    io.reads.push_back(Ok(state(Air75MainState::off(), side)));
    for _ in 0..6 {
        io.reads.push_back(Ok(state(Air75MainState::off(), side)));
    }
    let mut controller = Air75Controller::new(io);

    let error = controller
        .apply_and_verify(Air75MainState::blue())
        .unwrap_err()
        .to_string();
    assert!(error.contains("expected=03 1e 03 00 00 00 00 00 ff"));
    assert!(error.contains("actual=03 00 03 00 00 00 00 00 00"));
}

#[test]
/// 归一化提交后仍严格检查 RGB，不能把早先出现的缩放值直接认作匹配。
fn normalized_main_write_rejects_scaled_rgb_readback() {
    let side = [2, 30, 3, 0, 0, 0, 255, 0];
    let scaled = Air75MainState::from_bytes([3, 30, 3, 0, 0, 0, 0, 0, 77]);
    let mut io = FakeIo::default();
    io.reads.push_back(Ok(state(Air75MainState::off(), side)));
    for _ in 0..6 {
        io.reads.push_back(Ok(state(scaled, side)));
    }
    let mut controller = Air75Controller::new(io);
    let error = controller.apply_and_verify(Air75MainState::blue()).unwrap_err();
    assert!(format!("{error:#}").contains("actual=03 1e 03 00 00 00 00 00 4d"));
    assert_eq!(controller.into_inner().writes, vec![
        (0, vec![3, 100, 3, 0, 0, 0, 0, 0, 255], 0),
        (1, vec![30], 0),
    ]);
}

#[test]
/// 颜色使用满亮度基准后，恢复仍必须提交原来的低亮度而非保留 100%。
fn main_restore_keeps_original_low_brightness() {
    let original = Air75MainState::from_bytes([4, 37, 3, 0, 0, 0, 240, 0, 255]);
    let side = [2, 30, 3, 0, 0, 0, 255, 0];
    let mut io = FakeIo::default();
    for main in [original, Air75MainState::blue(), original] {
        io.reads.push_back(Ok(state(main, side)));
    }
    let mut controller = Air75Controller::new(io);
    controller.set_mode_handle(1);
    controller.test_main_once(Air75MainState::blue(), |_| Ok(())).unwrap();
    let io = controller.into_inner();
    assert_eq!(io.writes[2], (0, vec![4, 100, 3, 0, 0, 0, 240, 0, 255], 1));
    assert_eq!(io.writes[3], (1, vec![37], 1));
}

#[test]
fn apply_rejects_side_light_changes() {
    let mut io = FakeIo::default();
    io.reads.push_back(Ok(state(Air75MainState::off(), [1; 8])));
    io.reads.push_back(Ok(state(Air75MainState::blue(), [2; 8])));
    let mut controller = Air75Controller::new(io);

    assert!(controller.apply_and_verify(Air75MainState::blue()).is_err());
}

#[test]
/// 保留旧四色入口失败时尝试熄灭的行为。
fn exercise_attempts_to_turn_off_after_a_failed_step() {
    let side = [3; 8];
    let mut io = FakeIo::default();
    io.reads.push_back(Ok(state(Air75MainState::off(), side)));
    io.reads.push_back(Err(anyhow::anyhow!("模拟回读失败")));
    io.reads.push_back(Ok(state(Air75MainState::off(), side)));
    let mut controller = Air75Controller::new(io);

    assert!(controller.exercise(|_| {}).is_err());
    let io = controller.into_inner();
    assert_eq!(io.writes[2].1, vec![3, 100, 3, 0, 0, 0, 0, 0, 0]);
    assert_eq!(io.writes[3].1, vec![0]);
}

#[test]
/// 对照官方字段核对侧灯写入和亮度提交地址。
fn side_exercise_writes_side_state_then_air75_brightness_commit() {
    let main = Air75MainState::steady(100, 128, 0, 128).unwrap();
    let original = Air75SideState::from_bytes([2, 30, 3, 0, 0, 0, 255, 0]);
    let steps = Air75Controller::<FakeIo>::side_exercise_states();
    let mut io = FakeIo::default();
    io.reads.push_back(Ok(side_state(main, original)));
    for side in steps {
        io.reads.push_back(Ok(side_state(main, side)));
    }
    io.reads.push_back(Ok(side_state(main, original)));

    let mut controller = Air75Controller::new(io);
    controller.exercise_side(|_| {}).unwrap();
    let io = controller.into_inner();

    assert_eq!(io.writes.len(), 10);
    for writes in io.writes.chunks_exact(2) {
        assert_eq!((writes[0].0, writes[0].1.len(), writes[0].2), (9, 8, 0));
        assert_eq!((writes[1].0, writes[1].1.as_slice(), writes[1].2), (10, &writes[0].1[1..2], 0));
    }
    assert_eq!(io.writes[8].1, original.as_bytes());
}

#[test]
fn side_write_rejects_any_main_area_change() {
    let before = Air75MainState::steady(100, 128, 0, 128).unwrap();
    let after = Air75MainState::steady(100, 0, 0, 255).unwrap();
    let side = Air75SideState::blue();
    let mut io = FakeIo::default();
    io.reads.push_back(Ok(side_state(before, Air75SideState::off())));
    io.reads.push_back(Ok(side_state(after, side)));
    let mut controller = Air75Controller::new(io);

    assert!(controller.apply_side_and_verify(side).is_err());
}

#[test]
/// 主键区颜色索引也必须保护，不能把意外改变当作固件正常行为。
fn side_write_rejects_main_color_index_changes() {
    let side = Air75SideState::blue();
    let mut before = [0_u8; 17];
    before[..9].copy_from_slice(&[3, 0, 3, 0, 1, 0xbb, 0, 0, 0]);
    let mut after = before;
    after[5] = 1;
    after[9..].copy_from_slice(side.as_bytes());
    let mut io = FakeIo::default();
    io.reads.push_back(Ok(before));
    io.reads.push_back(Ok(after));
    let mut controller = Air75Controller::new(io);

    assert!(controller.apply_side_and_verify(side).is_err());
}

#[test]
/// 单色测试只操作选定模式的侧灯，并恢复完整侧灯快照。
fn single_side_test_restores_original_in_selected_mode() {
    let main = Air75MainState::off();
    let original = Air75SideState::from_bytes([2, 29, 3, 0, 0xfc, 22, 22, 255]);
    let mut io = FakeIo::default();
    io.reads.push_back(Ok(side_state(main, original)));
    io.reads.push_back(Ok(side_state(main, Air75SideState::blue())));
    io.reads.push_back(Ok(side_state(main, original)));
    let mut controller = Air75Controller::new(io);
    controller.set_mode_handle(1);
    let mut observations = 0;
    controller.test_side_once(Air75SideState::blue(), |_| {
        observations += 1;
        Ok(())
    }).unwrap();
    assert_eq!(observations, 1);
    assert_eq!(controller.into_inner().writes, vec![
        (9, vec![2, 30, 3, 0, 0, 0, 0, 255], 1),
        (10, vec![30], 1),
        (9, original.as_bytes().to_vec(), 1),
        (10, vec![29], 1),
    ]);
}

#[test]
/// 用户反馈异常时仍执行恢复，且不把测试错误改成成功。
fn single_side_test_restores_after_observation_failure() {
    let main = Air75MainState::off();
    let original = Air75SideState::green();
    let mut io = FakeIo::default();
    io.reads.push_back(Ok(side_state(main, original)));
    io.reads.push_back(Ok(side_state(main, Air75SideState::blue())));
    io.reads.push_back(Ok(side_state(main, original)));
    let mut controller = Air75Controller::new(io);
    let error = controller.test_side_once(Air75SideState::blue(), |_| {
        anyhow::bail!("实体颜色错误")
    }).unwrap_err();
    assert!(format!("{error:#}").contains("实体颜色错误"));
    assert_eq!(controller.into_inner().writes[2].1, original.as_bytes());
}

#[test]
/// 回读失败时也尝试恢复，并同时报告原始错误和恢复错误。
fn single_side_test_reports_both_readback_and_restore_failure() {
    let original = Air75SideState::green();
    let mut io = FakeIo::default();
    io.reads.push_back(Ok(side_state(Air75MainState::off(), original)));
    io.reads.push_back(Err(anyhow::anyhow!("蓝色回读失败")));
    io.reads.push_back(Err(anyhow::anyhow!("恢复回读失败")));
    let mut controller = Air75Controller::new(io);
    let error = controller.test_side_once(Air75SideState::blue(), |_| {
        panic!("回读失败时不应请求观察")
    }).unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("蓝色回读失败"));
    assert!(message.contains("恢复回读失败"));
    assert_eq!(controller.into_inner().writes[2].1, original.as_bytes());
}

#[test]
/// 多步观察失败后必须跳过剩余步骤，恢复最初快照而非上一步状态。
fn side_sequence_stops_on_failure_and_restores_initial_snapshot() {
    let main = Air75MainState::off();
    let original = Air75SideState::green();
    let steps = [10, 60, 0].map(|brightness| {
        Air75SideState::from_bytes([2, brightness, 3, 0, 0, 0, 0, 255])
    });
    let mut io = FakeIo::default();
    for side in [original, steps[0], steps[1], original] {
        io.reads.push_back(Ok(side_state(main, side)));
    }
    let mut controller = Air75Controller::new(io);
    let mut observations = 0;
    let error = controller.test_side_states(&steps, |_| {
        observations += 1;
        anyhow::ensure!(observations < 2, "亮度没有变化");
        Ok(())
    }).unwrap_err();
    assert!(format!("{error:#}").contains("亮度没有变化"));
    assert_eq!(observations, 2);
    let io = controller.into_inner();
    assert_eq!(io.writes.len(), 6);
    assert_eq!(io.writes[4].1, original.as_bytes());
    assert_eq!(io.writes[5].1, vec![30]);
}

#[test]
/// 在 Windows 模式只写主键区，并完整恢复包含原动态模式的快照。
fn single_main_test_preserves_side_and_restores_original_effect() {
    let original = Air75MainState::from_bytes([4, 100, 3, 0, 0, 0, 240, 0, 255]);
    let side = [2, 30, 3, 0, 0, 0, 255, 0];
    let mut io = FakeIo::default();
    for main in [original, Air75MainState::blue(), original] {
        io.reads.push_back(Ok(state(main, side)));
    }
    let mut controller = Air75Controller::new(io);
    controller.set_mode_handle(1);
    let mut observed = false;
    controller.test_main_once(Air75MainState::blue(), |_| {
        observed = true;
        Ok(())
    }).unwrap();
    assert!(observed);
    assert_eq!(controller.into_inner().writes, vec![
        (0, vec![3, 100, 3, 0, 0, 0, 0, 0, 255], 1),
        (1, vec![30], 1),
        (0, original.as_bytes().to_vec(), 1),
        (1, vec![100], 1),
    ]);
    assert!(Air75MainState::steady(101, 0, 0, 255).is_err());
}

#[test]
/// 人工观察失败时必须恢复原灯效，不能仅熄灭主键区。
fn single_main_test_restores_after_observation_failure() {
    let original = Air75MainState::from_bytes([4, 100, 3, 0, 0, 0, 240, 0, 255]);
    let side = [2, 30, 3, 0, 0, 0, 255, 0];
    let mut io = FakeIo::default();
    for main in [original, Air75MainState::blue(), original] {
        io.reads.push_back(Ok(state(main, side)));
    }
    let mut controller = Air75Controller::new(io);
    let error = controller.test_main_once(Air75MainState::blue(), |_| {
        anyhow::bail!("主键区不是蓝色")
    }).unwrap_err();
    assert!(format!("{error:#}").contains("主键区不是蓝色"));
    assert_eq!(controller.into_inner().writes[2].1, original.as_bytes());
}

#[test]
/// 主键区回读和恢复同时失败时，两条错误都必须保留。
fn single_main_test_reports_readback_and_restore_failure() {
    let original = Air75MainState::from_bytes([4, 100, 3, 0, 0, 0, 240, 0, 255]);
    let mut io = FakeIo::default();
    io.reads.push_back(Ok(state(original, [2, 30, 3, 0, 0, 0, 255, 0])));
    io.reads.push_back(Err(anyhow::anyhow!("主键区回读失败")));
    io.reads.push_back(Err(anyhow::anyhow!("主键区恢复失败")));
    let mut controller = Air75Controller::new(io);
    let error = controller.test_main_once(Air75MainState::blue(), |_| {
        panic!("回读失败时不应请求观察")
    }).unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("主键区回读失败"));
    assert!(message.contains("主键区恢复失败"));
    assert_eq!(controller.into_inner().writes[2].1, original.as_bytes());
}

#[test]
/// 设备先返回上一灯效时只继续回读，不再提交第二次写入。
fn main_write_waits_for_state_to_settle_without_rewriting() {
    let original = Air75MainState::from_bytes([4, 100, 3, 0, 0, 0, 240, 0, 255]);
    let side = [2, 30, 3, 0, 0, 0, 255, 0];
    let mut io = FakeIo::default();
    for main in [original, original, Air75MainState::blue()] {
        io.reads.push_back(Ok(state(main, side)));
    }
    let mut controller = Air75Controller::new(io);
    controller.apply_and_verify(Air75MainState::blue()).unwrap();
    assert_eq!(controller.into_inner().writes.len(), 2);
}

#[test]
/// 分阶段诊断保留真实缩放值，人工反馈失败后仍恢复且不再写诊断数据。
fn main_diagnostic_records_stages_and_restores_after_visual_failure() {
    let original = Air75MainState::from_bytes([4, 100, 3, 0, 0, 0, 240, 0, 255]);
    let scaled = Air75MainState::from_bytes([3, 30, 3, 0, 0, 0, 0, 0, 77]);
    let side = [2, 30, 3, 0, 0, 0, 255, 0];
    let mut io = FakeIo::default();
    for main in [original, Air75MainState::blue(), scaled, scaled, original] {
        io.reads.push_back(Ok(state(main, side)));
    }
    let mut controller = Air75Controller::new(io);
    controller.set_mode_handle(1);
    let mut stages = Vec::new();
    let error = controller.diagnose_main_once(Air75MainState::blue(), |stage, actual| {
        stages.push((stage.to_owned(), actual[8]));
        anyhow::ensure!(stage != "settled", "实体颜色异常");
        Ok(())
    }).unwrap_err();
    assert!(format!("{error:#}").contains("实体颜色异常"));
    assert_eq!(stages, vec![("block".to_owned(), 255), ("brightness".to_owned(), 77), ("settled".to_owned(), 77)]);
    assert_eq!(controller.into_inner().writes, vec![
        (0, Air75MainState::blue().as_bytes().to_vec(), 1),
        (1, vec![30], 1),
        (0, original.as_bytes().to_vec(), 1),
        (1, vec![100], 1),
    ]);
}

#[test]
/// 多步测试完成或观察失败都恢复最初动态灯效，失败时跳过余下步骤。
fn main_brightness_sequence_restores_initial_snapshot_and_stops_on_failure() {
    let original = Air75MainState::from_bytes([4, 100, 3, 0, 0, 0, 240, 0, 255]);
    let side = [2, 30, 3, 0, 0, 0, 255, 0];
    let steps = [
        Air75MainState::steady(10, 0, 0, 255).unwrap(),
        Air75MainState::steady(60, 0, 0, 255).unwrap(),
        Air75MainState::off(),
    ];
    for fail_at_second in [false, true] {
        let count = if fail_at_second { 2 } else { 3 };
        let mut io = FakeIo::default();
        io.reads.push_back(Ok(state(original, side)));
        for &main in &steps[..count] {
            io.reads.push_back(Ok(state(main, side)));
        }
        io.reads.push_back(Ok(state(original, side)));
        let mut controller = Air75Controller::new(io);
        controller.set_mode_handle(1);
        let mut observations = 0;
        let result = controller.test_main_states(&steps, |_| {
            observations += 1;
            anyhow::ensure!(!fail_at_second || observations != 2, "亮度未变化");
            Ok(())
        });
        assert_eq!(result.is_err(), fail_at_second);
        assert_eq!(observations, count);
        let io = controller.into_inner();
        assert_eq!(io.writes.len(), (count + 1) * 2);
        for (writes, expected) in io.writes.chunks_exact(2).zip(steps[..count].iter().chain(std::iter::once(&original))) {
            let mut color = *expected.as_bytes();
            color[1] = 100;
            assert_eq!(writes[0], (0, color.to_vec(), 1));
            assert_eq!(writes[1], (1, vec![expected.as_bytes()[1]], 1));
        }
    }
}

#[test]
/// 诊断保留缩放 RGB，亮度序列不重写颜色，观察失败后跳过余下步骤并恢复。
fn independent_brightness_diagnostic_preserves_readback_and_restores() {
    let original = Air75MainState::from_bytes([4, 100, 3, 0, 0, 0, 240, 0, 255]);
    let side = [2, 30, 3, 0, 0, 0, 255, 0];
    let steps = [(100, 255), (10, 26), (60, 153), (0, 0)];
    for fail_at_second in [false, true] {
        let count = if fail_at_second { 2 } else { 4 };
        let mut io = FakeIo::default();
        io.reads.push_back(Ok(state(original, side)));
        for &(brightness, blue) in &steps[..count] {
            io.reads.push_back(Ok(state(
                Air75MainState::from_bytes([3, brightness, 3, 0, 0, 0, 0, 0, blue]), side,
            )));
        }
        io.reads.push_back(Ok(state(original, side)));
        let mut controller = Air75Controller::new(io);
        controller.set_mode_handle(1);
        let mut observations = Vec::new();
        let result = controller.diagnose_main_brightness(|brightness, actual| {
            observations.push((brightness, actual[8]));
            anyhow::ensure!(!fail_at_second || brightness != 10, "实体亮度未变化");
            Ok(())
        });
        assert_eq!(observations, steps[..count]);
        assert_eq!(result.is_err(), fail_at_second);
        if let Err(error) = result {
            assert!(format!("{error:#}").contains("实体亮度未变化"));
        }
        let io = controller.into_inner();
        assert_eq!(io.writes.len(), count + 3);
        assert_eq!(io.writes[0], (0, vec![3, 100, 3, 0, 0, 0, 0, 0, 255], 1));
        for (index, &(brightness, _)) in steps[..count].iter().enumerate() {
            assert_eq!(io.writes[index + 1], (1, vec![brightness], 1));
        }
        assert_eq!(io.writes[count + 1], (0, original.as_bytes().to_vec(), 1));
        assert_eq!(io.writes[count + 2], (1, vec![100], 1));
    }
}

#[test]
/// 侧灯异常或通信失败时跳过观察与后续亮度，并同时保留恢复失败原因。
fn independent_brightness_diagnostic_stops_on_io_or_side_failure() {
    let original = Air75MainState::from_bytes([4, 100, 3, 0, 0, 0, 240, 0, 255]);
    let side = [2, 30, 3, 0, 0, 0, 255, 0];
    for side_changed in [false, true] {
        let mut io = FakeIo::default();
        io.reads.push_back(Ok(state(original, side)));
        io.reads.push_back(if side_changed {
            Ok(state(original, [0; 8]))
        } else {
            Err(anyhow::anyhow!("诊断通信失败"))
        });
        io.reads.push_back(Err(anyhow::anyhow!("恢复通信失败")));
        let mut controller = Air75Controller::new(io);
        let error = controller.diagnose_main_brightness(|_, _| {
            panic!("侧灯异常或读取失败时不应请求观察")
        }).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains(if side_changed { "侧灯状态发生变化" } else { "诊断通信失败" }));
        assert!(message.contains("恢复通信失败"));
        let io = controller.into_inner();
        assert_eq!(io.writes.len(), 4);
        assert_eq!(io.writes[2].1, original.as_bytes());
    }
}
