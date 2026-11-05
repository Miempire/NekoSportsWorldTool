//! 设备传感器档案：按机型生成可信的传感器清单。
//!
//! 用途：模拟真机 `SensorManager.getSensorList()` 的结果。缺少传感器（尤其加速度/
//! 陀螺仪/步数）是模拟器检测的典型信号，所以这里按机型给出完整、参数自洽的清单。
//!
//! 同一设备结果稳定：seed = MD5(device_id + "|" + 机型 + "|sensors-v1")。

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// 单个传感器条目，字段对齐 Android `Sensor` 常用属性。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SensorInfo {
    /// 传感器名（如 "BMI160 Accelerometer"）。
    pub name: String,
    pub vendor: String,
    /// Android `Sensor.TYPE_*`；iOS 用顺序号。
    pub type_id: i32,
    pub type_name: String,
    pub version: i32,
    /// 量程（m/s²、rad/s、µT、hPa、lux、cm）。
    pub max_range: f32,
    pub resolution: f32,
    /// 功耗（mA）。
    pub power_ma: f32,
    /// 最小采样间隔（微秒），0 表示按事件触发。
    pub min_delay_us: i32,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct SensorProfile {
    pub sensors: Vec<SensorInfo>,
}

fn sensor(
    name: &str,
    vendor: &str,
    type_id: i32,
    type_name: &str,
    max_range: f32,
    resolution: f32,
    power_ma: f32,
    min_delay_us: i32,
) -> SensorInfo {
    SensorInfo {
        name: name.into(),
        vendor: vendor.into(),
        type_id,
        type_name: type_name.into(),
        version: 1,
        max_range,
        resolution,
        power_ma,
        min_delay_us,
    }
}

fn is_flagship(model: &str) -> bool {
    let m = model.to_lowercase();
    [
        "pro", "max", "ultra", "plus", "k50", "k60", "x100", "find", "mate", "pixel 7",
        "pixel 8", "iphone 1",
    ]
    .iter()
    .any(|k| m.contains(k))
}

fn is_budget(model: &str) -> bool {
    let m = model.to_lowercase();
    ["se", "lite", "play", "a0", "a1", "a2", "a3", "c1"]
        .iter()
        .any(|k| m.contains(k))
}

fn android_profile(model: &str, rng: &mut StdRng) -> Vec<SensorInfo> {
    let chip = match rng.gen_range(0..3) {
        0 => ("Bosch", "BMI160"),
        1 => ("STMicroelectronics", "LSM6DSO"),
        _ => ("InvenSense", "ICM42605"),
    };
    let mag = ("AKM", "AK09918");
    let light = ("Sensortek", "STK3338");
    let baro = ("Bosch", "BMP280");
    let virtual_vendor = "QTI";

    let mut list = Vec::with_capacity(16);
    list.push(sensor(
        &format!("{} Accelerometer", chip.1),
        chip.0,
        1,
        "android.sensor.accelerometer",
        78.4532,
        0.0023956298,
        0.35,
        5000,
    ));
    list.push(sensor(
        &format!("{} Gyroscope", chip.1),
        chip.0,
        4,
        "android.sensor.gyroscope",
        34.906586,
        0.0010652642,
        0.5,
        5000,
    ));
    list.push(sensor(
        &format!("{} Magnetometer", mag.1),
        mag.0,
        2,
        "android.sensor.magnetic_field",
        4912.0,
        0.15,
        0.5,
        20000,
    ));
    list.push(sensor(
        &format!("{} Light", light.1),
        light.0,
        5,
        "android.sensor.light",
        65535.0,
        1.0,
        0.1,
        0,
    ));
    list.push(sensor(
        &format!("{} Proximity", light.1),
        light.0,
        8,
        "android.sensor.proximity",
        5.0,
        0.0,
        0.1,
        0,
    ));

    if is_flagship(model) || rng.gen_bool(0.35) {
        list.push(sensor(
            &format!("{} Pressure", baro.1),
            baro.0,
            6,
            "android.sensor.pressure",
            1100.0,
            0.01,
            0.5,
            20000,
        ));
    }

    list.push(sensor(
        "Gravity",
        virtual_vendor,
        9,
        "android.sensor.gravity",
        78.4532,
        0.0,
        0.0,
        0,
    ));
    list.push(sensor(
        "Linear Acceleration",
        virtual_vendor,
        10,
        "android.sensor.linear_acceleration",
        78.4532,
        0.0,
        0.0,
        0,
    ));
    list.push(sensor(
        "Rotation Vector",
        virtual_vendor,
        11,
        "android.sensor.rotation_vector",
        1.0,
        0.0,
        0.0,
        0,
    ));
    list.push(sensor(
        "Game Rotation Vector",
        virtual_vendor,
        15,
        "android.sensor.game_rotation_vector",
        1.0,
        0.0,
        0.0,
        0,
    ));
    list.push(sensor(
        "Step Counter",
        virtual_vendor,
        19,
        "android.sensor.step_counter",
        0.0,
        0.0,
        0.13,
        0,
    ));
    list.push(sensor(
        "Step Detector",
        virtual_vendor,
        18,
        "android.sensor.step_detector",
        0.0,
        0.0,
        0.13,
        0,
    ));
    list.push(sensor(
        "Significant Motion",
        virtual_vendor,
        17,
        "android.sensor.significant_motion",
        1.0,
        0.0,
        0.13,
        0,
    ));

    if !is_budget(model) {
        list.push(sensor(
            "Device Orientation",
            virtual_vendor,
            27,
            "android.sensor.device_orientation",
            1.0,
            0.0,
            0.0,
            0,
        ));
        list.push(sensor(
            "Stationary Detect",
            virtual_vendor,
            29,
            "android.sensor.stationary_detect",
            1.0,
            0.0,
            0.13,
            0,
        ));
    }
    if is_flagship(model) {
        list.push(sensor(
            "Pick Up Gesture",
            virtual_vendor,
            25,
            "android.sensor.pick_up_gesture",
            1.0,
            0.0,
            0.13,
            0,
        ));
    }
    list
}

fn ios_profile(rng: &mut StdRng) -> Vec<SensorInfo> {
    let mut list = vec![
        sensor("Accelerometer", "Apple", 1, "Accelerometer", 16.0, 0.0001, 0.2, 10000),
        sensor("Gyroscope", "Apple", 2, "Gyroscope", 34.9, 0.0001, 0.3, 10000),
        sensor("Magnetometer", "Apple", 3, "Magnetometer", 4912.0, 0.1, 0.2, 20000),
        sensor("Device Motion", "Apple", 4, "DeviceMotion", 1.0, 0.0, 0.0, 10000),
        sensor("Barometer", "Bosch", 5, "Barometer", 1100.0, 0.01, 0.5, 20000),
        sensor("Proximity", "Apple", 6, "Proximity", 5.0, 0.0, 0.1, 0),
        sensor("Ambient Light", "Apple", 7, "AmbientLight", 65535.0, 1.0, 0.1, 0),
        sensor("Step Counter", "Apple", 8, "StepCounter", 0.0, 0.0, 0.1, 0),
    ];
    if rng.gen_bool(0.6) {
        list.push(sensor("Step Detector", "Apple", 9, "StepDetector", 0.0, 0.0, 0.1, 0));
    }
    list
}

/// 按设备身份生成（同一 DeviceId + 机型结果稳定）。
pub fn generate(model: &str, device_id: &str, platform: &str) -> SensorProfile {
    let seed_text = format!("{device_id}|{model}|sensors-v1");
    let digest = crate::crypto::envelope::md5_hex(seed_text.as_bytes());
    let seed = u64::from_str_radix(&digest[..16], 16).unwrap_or(0);
    let mut rng = StdRng::seed_from_u64(seed);

    let sensors = if platform == "android" {
        android_profile(model, &mut rng)
    } else {
        ios_profile(&mut rng)
    };
    SensorProfile { sensors }
}

/// 用当前身份生成传感器档案。
pub fn generate_for(identity: &crate::crypto::header::HeaderIdentity) -> SensorProfile {
    generate(&identity.device_name, &identity.device_id, &identity.platform)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_same_device() {
        let a = generate("iPhone 14", "DEV-1", "ios");
        let b = generate("iPhone 14", "DEV-1", "ios");
        assert_eq!(a, b);
        // iOS 档案只有少量随机项，不同设备可能恰好相同，这里只验证可复现性。
        let _ = generate("iPhone 14", "DEV-2", "ios");
    }

    #[test]
    fn android_has_core_sensors() {
        let p = generate("22081212C", "DEV-X", "android");
        assert!(p.sensors.len() >= 10, "传感器太少: {}", p.sensors.len());
        let names: Vec<&str> = p.sensors.iter().map(|s| s.type_name.as_str()).collect();
        for need in [
            "android.sensor.accelerometer",
            "android.sensor.gyroscope",
            "android.sensor.magnetic_field",
            "android.sensor.step_counter",
        ] {
            assert!(names.contains(&need), "缺少 {need}");
        }
        for s in &p.sensors {
            assert!(!s.name.is_empty() && !s.vendor.is_empty());
            assert!(s.max_range >= 0.0);
        }
    }

    #[test]
    fn ios_profile_is_apple_like() {
        let p = generate("iPhone 15", "DEV-Y", "ios");
        assert!(p.sensors.iter().any(|s| s.type_name == "Accelerometer"));
        assert!(p.sensors.iter().all(|s| !s.vendor.is_empty()));
    }
}
