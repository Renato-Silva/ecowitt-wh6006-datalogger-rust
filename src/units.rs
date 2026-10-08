//! Unit selection (bytes 2..6 of the 0x05 settings reply) and conversion
//! from device units (0.1 °C, 0.1 hPa, 0.1 m/s, 0.1 mm, 0.1 lux) for display.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub struct Units {
    /// 0 = °C, 1 = °F
    pub temp: u8,
    /// 0 = hPa, 1 = mmHg, 2 = inHg
    pub pressure: u8,
    /// 0 = m/s, 1 = km/h, 2 = mph, 3 = knots, 4 = bft (5+ = ft/s in the app)
    pub wind: u8,
    /// 0 = mm, 1 = inch
    pub rain: u8,
    /// 0 = lux, 1 = fc, 2 = W/m²
    pub light: u8,
}

pub const TEMP_NAMES: &[&str] = &["C", "F"];
pub const PRESSURE_NAMES: &[&str] = &["hPa", "mmHg", "inHg"];
pub const WIND_NAMES: &[&str] = &["m/s", "km/h", "mph", "knots", "bft"];
pub const RAIN_NAMES: &[&str] = &["mm", "inch"];
pub const LIGHT_NAMES: &[&str] = &["lux", "fc", "w/m2"];

/// Parse a unit name (case-insensitive) into its index in `names`.
pub fn parse_unit(names: &[&str], s: &str) -> Option<u8> {
    names
        .iter()
        .position(|n| n.eq_ignore_ascii_case(s))
        .map(|i| i as u8)
}

impl Units {
    pub const METRIC: Units = Units {
        temp: 0,
        pressure: 0,
        wind: 0,
        rain: 0,
        light: 0,
    };

    /// Temperature from raw s16 tenths of °C. 0x7FFF = no data.
    pub fn temp(&self, raw: i16) -> String {
        if raw == 0x7FFF {
            return "--.-".into();
        }
        let c = raw as f64 / 10.0;
        if self.temp == 0 {
            format!("{c:.1} °C")
        } else {
            format!("{:.1} °F", c * 1.8 + 32.0)
        }
    }

    /// Pressure from raw u16 tenths of hPa.
    pub fn pressure(&self, raw: u16) -> String {
        if raw == 0xFFFF || raw == 0x7FFF {
            return "--.-".into();
        }
        let hpa = raw as f64 / 10.0;
        match self.pressure {
            0 => format!("{hpa:.1} hPa"),
            2 => format!("{:.2} inHg", hpa / 33.86388),
            _ => format!("{:.1} mmHg", hpa / 1.33322),
        }
    }

    /// Wind speed from raw u16 tenths of m/s.
    pub fn wind(&self, raw: u16) -> String {
        if raw == 0xFFFF || raw == 0x7FFF {
            return "--.-".into();
        }
        let ms = raw as f64 / 10.0;
        match self.wind {
            0 => format!("{ms:.1} m/s"),
            1 => format!("{:.1} km/h", ms * 3.6),
            2 => format!("{:.1} mph", ms * 2.236936),
            3 => format!("{:.1} knots", ms * 1.943844),
            4 => format!("{} bft", beaufort(raw)),
            _ => format!("{:.1} ft/s", ms * 3.2808),
        }
    }

    /// Rain from raw tenths of mm. `None` = no data.
    pub fn rain(&self, raw: Option<u32>, per_hour: bool) -> String {
        let Some(raw) = raw else { return "--.-".into() };
        let mm = raw as f64 / 10.0;
        let suffix = if per_hour { "/h" } else { "" };
        if self.rain == 0 {
            format!("{mm:.1} mm{suffix}")
        } else {
            format!("{:.2} inch{suffix}", mm / 25.4)
        }
    }

    /// Light from raw u32 tenths of lux. Exactly 0x00FFFFFF = no data.
    pub fn light(&self, raw: u32) -> String {
        if raw == 0x00FF_FFFF {
            return "--.-".into();
        }
        let lux = raw as f64 / 10.0;
        match self.light {
            0 => format!("{lux:.1} lux"),
            1 => format!("{:.2} fc", lux / 10.76391),
            _ => format!("{:.3} w/m2", lux / 126.7),
        }
    }
}

/// Beaufort scale from raw tenths of m/s, using the app's thresholds.
pub fn beaufort(raw: u16) -> u8 {
    const T: [u16; 12] = [3, 15, 34, 54, 79, 107, 138, 171, 207, 244, 284, 326];
    T.iter().position(|&t| raw < t).unwrap_or(12) as u8
}

pub fn pct(raw: u8) -> String {
    if raw == 0xFF {
        "--".into()
    } else {
        format!("{raw} %")
    }
}

pub fn pm25(raw: u16) -> String {
    if raw == 0xFFFF || raw == 0x3FFF {
        "--.-".into()
    } else {
        format!("{:.1} ug/m3", raw as f64 / 10.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        let u = Units { temp: 1, pressure: 2, wind: 1, rain: 1, light: 1 };
        assert_eq!(u.temp(200), "68.0 °F");
        assert_eq!(u.temp(0x7FFF), "--.-");
        assert_eq!(u.pressure(10132), "29.92 inHg");
        assert_eq!(u.wind(100), "36.0 km/h");
        assert_eq!(u.rain(Some(254), false), "1.00 inch");
        assert_eq!(Units::METRIC.rain(None, false), "--.-");
        assert_eq!(Units::METRIC.light(0x00FF_FFFF), "--.-");
        assert_eq!(beaufort(0), 0);
        assert_eq!(beaufort(326), 12);
        assert_eq!(beaufort(100), 5);
    }

    #[test]
    fn unit_names_parse() {
        assert_eq!(parse_unit(PRESSURE_NAMES, "inhg"), Some(2));
        assert_eq!(parse_unit(WIND_NAMES, "nope"), None);
    }
}
