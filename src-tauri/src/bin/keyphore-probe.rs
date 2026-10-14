/// 保留原探针入口，并提供需要人工确认的分区灯光测试。
fn main() -> anyhow::Result<()> {
    if std::env::args().any(|argument| argument == "--diagnose-main-brightness") {
        return diagnose_main_brightness();
    }
    if std::env::args().any(|argument| argument == "--diagnose-main-single") {
        return diagnose_main_single();
    }
    if let Some(test) = std::env::args().find(|argument| matches!(argument.as_str(),
        "--test-main-single" | "--test-main-brightness" | "--test-main-off" | "--test-main-colors" | "--test-main-rgb"
    )) {
        return test_main_single(&test);
    }
    let brightness_test = std::env::args().any(|argument| argument == "--test-side-brightness");
    if brightness_test || std::env::args().any(|argument| argument == "--test-side-single") {
        return test_side(brightness_test);
    }
    for candidate in airpulse_lib::hid::enumerate_air75_candidates()? {
        println!(
            "candidate: product={:?}, bus={:?}, interface={}, usage={:04x}:{:04x}, path={}",
            candidate.product,
            candidate.bus,
            candidate.interface_number,
            candidate.usage_page,
            candidate.usage,
            candidate.path
        );
    }
    if std::env::args().any(|argument| argument == "--read-modes") {
        let (_, mut controller) = airpulse_lib::hid::connect_air75()?;
        for handle in [0, 1] {
            let state = controller.read_state_for_handle(handle)?;
            println!(
                "handle={handle}, state={}",
                state
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        return Ok(());
    }
    if std::env::args().any(|argument| argument == "--read-colors") {
        let (_, mut controller) = airpulse_lib::hid::connect_air75()?;
        for handle in [0, 1] {
            let colors = controller.read_color_table(handle)?;
            println!(
                "handle={handle}, tail20={}",
                colors[84 * 4..]
                    .chunks(4)
                    .map(|record| record
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<Vec<_>>()
                        .join(""))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        return Ok(());
    }
    if std::env::args().any(|argument| argument == "--exercise-side") {
        let (descriptor, mut controller) = airpulse_lib::hid::connect_air75()?;
        if std::env::args().any(|argument| argument == "--mode-zero") {
            controller.set_mode_handle(0);
        }
        println!("Air75 V3 双侧条形灯测试: interface={}", descriptor.interface_number);
        let names = ["蓝色", "橙色", "绿色", "熄灭"];
        let mut index = 0;
        controller.exercise_side(|_| {
            println!("步骤 {} 回读成功", names[index]);
            index += 1;
            std::thread::sleep(std::time::Duration::from_secs(2));
        })?;
        println!("已恢复原条形灯状态，主键区内容保持不变");
        return Ok(());
    }
    if std::env::args().any(|argument| argument == "--custom-info") {
        let (_, mut controller) = airpulse_lib::hid::connect_air75()?;
        let report = controller.custom_lighting_report()?;
        println!(
            "firmware={}, led_count={}",
            report
                .firmware
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" "),
            report.led_count
        );
        return Ok(());
    }
    if std::env::args().any(|argument| argument == "--exercise-custom") {
        let (descriptor, mut controller) = airpulse_lib::hid::connect_air75()?;
        println!("Air75 V3 D8 灯光测试: interface={}", descriptor.interface_number);
        let report = controller.exercise_custom(|name| println!("步骤 {name} 已写入"))?;
        println!(
            "firmware={}, led_count={}, 已恢复原灯效",
            report
                .firmware
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" "),
            report.led_count
        );
        return Ok(());
    }
    if std::env::args().any(|argument| argument == "--exercise") {
        let (descriptor, mut controller) = airpulse_lib::hid::connect_air75()?;
        println!("Air75 V3 灯光测试: interface={}", descriptor.interface_number);
        let names = ["蓝色", "橙色", "绿色", "熄灭"];
        let mut index = 0;
        controller.exercise(|_| {
            println!("步骤 {} 回读成功", names[index]);
            index += 1;
            std::thread::sleep(std::time::Duration::from_millis(800));
        })?;
        return Ok(());
    }
    let report = airpulse_lib::hid::probe_read_only()?;
    println!(
        "Air75 V3: interface={}, usage={:04x}:{:04x}",
        report.descriptor.interface_number, report.descriptor.usage_page, report.descriptor.usage
    );
    println!(
        "state={}",
        report
            .state
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    Ok(())
}

/// 逐步观察单色或亮度变化，复用同一套快照恢复和实体确认流程。
fn test_side(brightness_test: bool) -> anyhow::Result<()> {
    let plan = if brightness_test {
        "本次只测试蓝色侧灯：10% 暗亮、60% 较亮、0% 熄灭，每一步等你确认后再继续。"
    } else {
        "本次只把左右侧灯改为蓝色、亮度 30%。"
    };
    if !ask_yes_no(
        "侧灯测试准备",
        &format!("请先关闭 NuPhyIO，保持键盘 USB 连接，不要切换 Mac/Windows 开关。\n\n{plan}\n\n不写主键区；结束或反馈异常后尝试恢复测试前侧灯状态。准备好开始了吗？选择“否”将取消。"),
    )? {
        println!("用户取消，未写入灯光");
        return Ok(());
    }
    let (_, mut controller) = airpulse_lib::hid::connect_air75()?;
    let handle = controller.select_current_mode()?;
    let baseline = controller.read_state()?;
    println!("current_mode={handle}, baseline={baseline:02x?}");
    let steps: &[(u8, &str)] = if brightness_test {
        &[
            (10, "左右侧灯是否都显示较暗的蓝色？"),
            (60, "左右侧灯是否仍为蓝色，并且比上一步 10% 明显更亮？"),
            (0, "左右侧灯是否都已熄灭？"),
        ]
    } else {
        &[(30, "左右侧灯是否都显示蓝色？")]
    };
    // 固定 RGB，只改变亮度；零亮度时保留蓝色以便单独验证亮度字段。
    let states: Vec<_> = steps.iter().map(|&(brightness, _)| {
        airpulse_lib::lighting::Air75SideState::from_bytes([2, brightness, 3, 0, 0, 0, 0, 255])
    }).collect();
    let mut index = 0;
    let result = controller.test_side_states(&states, |_| {
        let (brightness, question) = steps[index];
        println!("侧灯 {brightness}% 参数回读匹配，等待实体观察");
        let observed = ask_yes_no(
            "请观察键盘",
            &format!("步骤 {}/{}：{question}\n\n主键区是否与测试前完全一致？\n\n两项都符合请选择“是”；任一异常请选择“否”，停止后续步骤并尝试恢复。", index + 1, steps.len()),
        )?;
        println!("side_brightness={brightness}, visual_confirmed={observed}");
        anyhow::ensure!(observed, "用户观察到侧灯或主键区显示异常");
        index += 1;
        Ok(())
    });
    match &result {
        Ok(()) => println!("测试与恢复参数回读均匹配"),
        Err(error) => println!("测试或恢复出现问题: {error:#}"),
    }
    let restored = ask_yes_no(
        "请确认恢复效果",
        &format!(
            "已执行恢复尝试。\n\n左右侧灯是否恢复到测试前的颜色 #{:02X}{:02X}{:02X}、亮度 {}%，且主键区保持不变？\n\n程序结果：{}",
            baseline[14], baseline[15], baseline[16], baseline[10],
            match &result {
                Ok(()) => "写入与恢复回读匹配".to_owned(),
                Err(error) => format!("{error:#}"),
            }
        ),
    )?;
    println!("restore_visual_confirmed={restored}");
    result?;
    anyhow::ensure!(restored, "用户观察到恢复效果异常，请通过 NuPhyIO 恢复侧灯");
    println!("侧灯测试通过：全部步骤与恢复效果均获人工确认");
    Ok(())
}

/// 在当前模式下逐步验证主键区颜色或亮度，复用同一初始快照恢复。
fn test_main_single(test: &str) -> anyhow::Result<()> {
    let plan = match test {
        "--test-main-rgb" => "本次测试整片主键区自选 RGB：紫色 #A020F0、青色 #12B4C8、白色 #FFFFFF，亮度均为 30%，最后熄灭。不是逐键独立设色；每步等待你观察后再继续。",
        "--test-main-colors" => "本次依次测试主键区静态蓝色 #0000FF、橙色 #FF8400、绿色 #00FF00，亮度均为 30%，最后用全黑状态熄灭。每一步等待你观察后再继续。",
        "--test-main-brightness" => "本次通过修改后的共用控制函数，依次测试静态蓝色 100%、10%、60%、0% 熄灭。每步单独设置颜色后提交亮度，等待你确认后再继续。",
        "--test-main-off" => "本次只测试主键区熄灭：亮度 0%，RGB 全黑 #000000。",
        _ => "本次只将主键区改为静态蓝色 #0000FF、亮度 30%。",
    };
    if !ask_yes_no(
        "主键区测试准备",
        &format!("请先按键唤醒键盘，关闭 NuPhyIO，保持 USB 连接，不要切换 Mac/Windows 开关。\n\n请记住主键区现在的颜色和动态效果。{plan}\n\n每次设置颜色可能短暂经过 100% 亮度；弹窗出现后再判断最终效果。不写侧灯；结束或反馈异常后尝试恢复原灯效。准备好开始了吗？选择“否”将取消。"),
    )? {
        println!("用户取消，未写入灯光");
        return Ok(());
    }
    let (_, mut controller) = airpulse_lib::hid::connect_air75()?;
    let handle = controller.select_current_mode()?;
    let baseline = controller.read_state()?;
    println!("current_mode={handle}, main_baseline={baseline:02x?}");
    let steps: &[(u8, &str)] = match test {
        "--test-main-rgb" => &[
            (30, "主键区是否显示稳定紫色？这次指定 RGB 为 #A020F0。"),
            (30, "主键区是否从紫色切换为稳定青色？这次指定 RGB 为 #12B4C8。"),
            (30, "主键区是否从青色切换为稳定白色？这次指定 RGB 为 #FFFFFF。"),
            (0, "主键区背光是否已经全部熄灭？"),
        ],
        "--test-main-colors" => &[
            (30, "主键区是否显示稳定蓝色，没有混入其他颜色？"),
            (30, "主键区是否已经从蓝色切换为稳定橙色？"),
            (30, "主键区是否已经从橙色切换为稳定绿色？"),
            (0, "主键区背光是否已经全部熄灭？"),
        ],
        "--test-main-brightness" => &[
            (100, "主键区是否显示稳定蓝色？请记住当前亮度。"),
            (10, "主键区是否仍为静态蓝色，并且比上一步 100% 明显变暗？"),
            (60, "主键区是否仍为静态蓝色，并且比上一步 10% 明显更亮？"),
            (0, "主键区背光是否全部熄灭？"),
        ],
        "--test-main-off" => &[(0, "主键区背光是否全部熄灭？")],
        _ => &[(30, "主键区是否为静态蓝色，没有彩虹或持续变色？")],
    };
    // 亮度序列在 0% 保留蓝色，独立验证熄灭是否由亮度提交生效。
    let states: Vec<_> = if test == "--test-main-rgb" {
        vec![
            airpulse_lib::lighting::Air75MainState::steady(30, 160, 32, 240)
                .expect("固定测试亮度有效"),
            airpulse_lib::lighting::Air75MainState::steady(30, 18, 180, 200)
                .expect("固定测试亮度有效"),
            airpulse_lib::lighting::Air75MainState::steady(30, 255, 255, 255)
                .expect("固定测试亮度有效"),
            airpulse_lib::lighting::Air75MainState::off(),
        ]
    } else if test == "--test-main-colors" {
        airpulse_lib::hid::Air75Controller::<airpulse_lib::hid::HidLightingIo>::exercise_states().to_vec()
    } else {
        steps.iter().map(|&(brightness, _)| {
            if test == "--test-main-off" {
                airpulse_lib::lighting::Air75MainState::off()
            } else {
                airpulse_lib::lighting::Air75MainState::steady(brightness, 0, 0, 255)
                    .expect("固定测试亮度有效")
            }
        }).collect()
    };
    let mut index = 0;
    let result = controller.test_main_states(&states, |main| {
        let (brightness, question) = steps[index];
        println!("主键区状态 {:02x?} 回读匹配，等待实体观察", main.as_bytes());
        let observed = ask_yes_no(
            "请观察主键区",
            &format!("步骤 {}/{}：{question}\n\n符合选“是”；异常选“否”，停止后续步骤并尝试恢复。", index + 1, steps.len()),
        )?;
        println!("main_brightness={brightness}, visual_confirmed={observed}");
        if matches!(test, "--test-main-colors" | "--test-main-rgb") {
            println!("main_color_step={}, state={:02x?}, visual_confirmed={observed}", index + 1, main.as_bytes());
        }
        if test == "--test-main-single" {
            println!("main_blue_visual_confirmed={observed}");
        }
        anyhow::ensure!(observed, "用户观察到主键区显示异常");
        let side_unchanged = ask_yes_no("请观察左右侧灯", "左右侧灯是否与本轮测试前完全一致？")?;
        println!("main_brightness={brightness}, side_visual_unchanged={side_unchanged}");
        anyhow::ensure!(side_unchanged, "用户观察到侧灯变化");
        index += 1;
        Ok(())
    });
    match &result {
        Ok(()) => println!("主键区测试与恢复参数回读均匹配，侧灯未变"),
        Err(error) => println!("主键区测试或恢复出现问题: {error:#}"),
    }
    let restored = ask_yes_no(
        "请确认主键区恢复效果",
        &format!(
            "已执行恢复尝试。主键区是否恢复测试前的颜色和动态效果，左右侧灯是否仍保持原样？\n\n保存的主键区模式={}，亮度={}%；侧灯颜色=#{:02X}{:02X}{:02X}，亮度={}% 。\n\n程序结果：{}",
            baseline[0], baseline[1], baseline[14], baseline[15], baseline[16], baseline[10],
            match &result {
                Ok(()) => "写入与恢复回读匹配".to_owned(),
                Err(error) => format!("{error:#}"),
            }
        ),
    )?;
    println!("main_restore_visual_confirmed={restored}");
    result?;
    anyhow::ensure!(restored, "用户观察到恢复效果异常，请通过 NuPhyIO 恢复主键区");
    println!("主键区测试通过：全部步骤与原灯效恢复均获人工确认");
    Ok(())
}

/// 记录主键区两次提交前后的实际参数，人工观察独立于严格回读判定。
fn diagnose_main_single() -> anyhow::Result<()> {
    if !ask_yes_no(
        "主键区分阶段诊断",
        "请关闭 NuPhyIO，保持 USB 连接，不要切换键盘模式。\n\n本次仍只测试主键区静态蓝色 #0000FF、亮度 30%，分别记录颜色块写入和亮度提交后的状态，不写侧灯。随后弹窗观察，并尝试恢复原灯效。\n\n准备好开始了吗？",
    )? {
        println!("用户取消，未写入灯光");
        return Ok(());
    }
    let (_, mut controller) = airpulse_lib::hid::connect_air75()?;
    let handle = controller.select_current_mode()?;
    let baseline = controller.read_state()?;
    println!("current_mode={handle}, diagnostic_baseline={baseline:02x?}");
    let result = controller.diagnose_main_once(airpulse_lib::lighting::Air75MainState::blue(), |stage, actual| {
        println!("main_stage={stage}, actual={actual:02x?}");
        if stage == "settled" {
            let observed = ask_yes_no(
                "请观察实际灯光",
                &format!("主键区现在是否为静态蓝色，没有彩虹或持续变色？左右侧灯是否与测试前完全一致？\n\n当前回读：亮度 {}%，RGB #{:02X}{:02X}{:02X}。此处只确认实体表现，不代表参数校验通过。\n\n两项符合选“是”，任一异常选“否”；随后尝试恢复原灯效。", actual[1], actual[6], actual[7], actual[8]),
            )?;
            println!("main_diagnostic_visual_confirmed={observed}");
            anyhow::ensure!(observed, "用户观察到主键区或侧灯异常");
        }
        Ok(())
    });
    println!("main_diagnostic_result={result:?}");
    let restored = ask_yes_no(
        "请确认诊断后恢复",
        &format!("已尝试恢复原主键区模式 {}、亮度 {}%，侧灯保持原样。\n\n主键区的颜色和动态效果是否恢复正常，侧灯是否未变？\n\n程序结果：{result:?}", baseline[0], baseline[1]),
    )?;
    println!("main_diagnostic_restore_visual_confirmed={restored}");
    result?;
    anyhow::ensure!(restored, "用户反馈恢复异常，请通过 NuPhyIO 恢复原灯效");
    Ok(())
}

/// 将亮度单独提交后的实体变化与原始回读分开记录，随后确认恢复。
fn diagnose_main_brightness() -> anyhow::Result<()> {
    if !ask_yes_no(
        "主键区亮度独立提交诊断",
        "请关闭 NuPhyIO，保持 USB 连接，不切换 Mac/Windows 开关。\n\n请记住当前灯效。本次先设静态蓝色 100%，再只提交亮度 10%、60%、0%，不再覆盖颜色块，不写侧灯。每一步弹窗等待你观察，异常时停止并尝试恢复原灯效。\n\n准备好开始了吗？",
    )? {
        println!("用户取消，未写入灯光");
        return Ok(());
    }
    let (_, mut controller) = airpulse_lib::hid::connect_air75()?;
    let handle = controller.select_current_mode()?;
    let baseline = controller.read_state()?;
    println!("current_mode={handle}, brightness_diagnostic_baseline={baseline:02x?}");
    let result = controller.diagnose_main_brightness(|brightness, actual| {
        println!("main_brightness_commit={brightness}, actual={actual:02x?}");
        let question = match brightness {
            100 => "主键区现在是否显示稳定的蓝色？请记住当前亮度。",
            10 => "与刚才的 100% 相比，主键区蓝色是否明显变暗？",
            60 => "与刚才的 10% 相比，主键区蓝色是否明显变亮？",
            _ => "主键区背光是否已经全部熄灭？如果仍亮着蓝色，请选“否”。",
        };
        let observed = ask_yes_no("请观察主键区亮度", question)?;
        println!("main_brightness_commit={brightness}, brightness_visual_confirmed={observed}");
        anyhow::ensure!(observed, "用户反馈主键区 {brightness}% 的实体效果不符合预期");
        let side_unchanged = ask_yes_no(
            "请观察左右侧灯",
            "左右侧灯是否与本轮测试前完全一致？是则继续，否将停止并尝试恢复。",
        )?;
        println!("main_brightness_commit={brightness}, side_visual_unchanged={side_unchanged}");
        anyhow::ensure!(side_unchanged, "用户反馈左右侧灯发生变化");
        Ok(())
    });
    println!("main_brightness_diagnostic_result={result:?}");
    let restored = ask_yes_no(
        "请确认主键区恢复",
        &format!("已尝试恢复测试前的主键区模式 {}、亮度 {}%。\n\n主键区原来的颜色和动态效果是否恢复正常？", baseline[0], baseline[1]),
    )?;
    println!("main_brightness_diagnostic_restore_visual_confirmed={restored}");
    result?;
    anyhow::ensure!(restored, "用户反馈恢复异常，请通过 NuPhyIO 恢复原灯效");
    println!("亮度独立提交诊断结束：已记录各步实体反馈与原始回读，未校准绝对亮度");
    Ok(())
}

/// 使用系统弹窗收集明确反馈，不把协议回读当作实体显示结果。
#[cfg(windows)]
fn ask_yes_no(title: &str, message: &str) -> anyhow::Result<bool> {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(window: *mut std::ffi::c_void, text: *const u16, caption: *const u16, flags: u32) -> i32;
    }
    let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let message: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    // 默认选“否”，并置于前台，避免误触发测试或遗漏观察提示。
    let answer = unsafe {
        MessageBoxW(std::ptr::null_mut(), message.as_ptr(), title.as_ptr(), 0x0005_0124)
    };
    match answer {
        6 => Ok(true),
        7 => Ok(false),
        _ => Err(anyhow::anyhow!("无法获取弹窗反馈: {}", std::io::Error::last_os_error())),
    }
}

/// 不在其他平台静默跳过必须的人工确认。
#[cfg(not(windows))]
fn ask_yes_no(_title: &str, _message: &str) -> anyhow::Result<bool> {
    anyhow::bail!("灯光弹窗测试仅支持 Windows")
}
