use std::ffi::CStr;
use std::path::PathBuf;
use std::time::Instant;

use gyroflow_finalcut::{
    GFError, GFOwnedBytes, GFStatus, gf_finalcut_error_free, gf_finalcut_instance_create,
    gf_finalcut_instance_free, gf_finalcut_instance_load_project,
    gf_finalcut_instance_load_project_payload, gf_finalcut_owned_bytes_free,
    gf_finalcut_project_payload_encode,
};

fn bridge_message(error: *mut GFError) -> String {
    if error.is_null() {
        return "unknown bridge error".to_string();
    }
    let message = unsafe {
        if (*error).message.is_null() {
            "unknown bridge error".to_string()
        } else {
            CStr::from_ptr((*error).message)
                .to_string_lossy()
                .into_owned()
        }
    };
    unsafe { gf_finalcut_error_free(error) };
    message
}

fn parse_arguments() -> Result<(PathBuf, usize), String> {
    let mut arguments = std::env::args_os().skip(1);
    let project = arguments.next().map(PathBuf::from).ok_or_else(|| {
        "usage: finalcut-payload-probe PROJECT.gyroflow [--iterations N]".to_string()
    })?;
    let mut iterations = 5_usize;
    while let Some(argument) = arguments.next() {
        if argument != "--iterations" {
            return Err(format!("unknown argument: {}", argument.to_string_lossy()));
        }
        let value = arguments
            .next()
            .ok_or_else(|| "--iterations requires a value".to_string())?;
        iterations = value
            .to_string_lossy()
            .parse()
            .map_err(|_| "iterations must be a positive integer".to_string())?;
    }
    if iterations == 0 {
        return Err("iterations must be positive".to_string());
    }
    if project.extension().and_then(|value| value.to_str()) != Some("gyroflow") {
        return Err("capacity probe accepts only a .gyroflow project".to_string());
    }
    Ok((project, iterations))
}

fn run() -> Result<serde_json::Value, String> {
    let (project_path, iterations) = parse_arguments()?;
    let project = std::fs::read(&project_path)
        .map_err(|error| format!("unable to read {}: {error}", project_path.display()))?;
    let mut error: *mut GFError = std::ptr::null_mut();
    let validator = unsafe { gf_finalcut_instance_create(&mut error) };
    if validator.is_null() {
        return Err(bridge_message(error));
    }
    let status = unsafe {
        gf_finalcut_instance_load_project(validator, project.as_ptr(), project.len(), &mut error)
    };
    unsafe { gf_finalcut_instance_free(validator) };
    if status != GFStatus::Ok {
        return Err(format!(
            "project validation failed: {}",
            bridge_message(error)
        ));
    }

    let mut encode_micros = Vec::with_capacity(iterations);
    let mut restore_micros = Vec::with_capacity(iterations);
    let mut payload_bytes = 0_usize;
    for _ in 0..iterations {
        let mut payload = GFOwnedBytes::default();
        error = std::ptr::null_mut();
        let encode_started = Instant::now();
        let status = unsafe {
            gf_finalcut_project_payload_encode(
                project.as_ptr(),
                project.len(),
                &mut payload,
                &mut error,
            )
        };
        encode_micros.push(encode_started.elapsed().as_micros() as u64);
        if status != GFStatus::Ok {
            return Err(format!(
                "payload encoding failed: {}",
                bridge_message(error)
            ));
        }
        payload_bytes = payload.len;

        let restored = unsafe { gf_finalcut_instance_create(&mut error) };
        if restored.is_null() {
            unsafe { gf_finalcut_owned_bytes_free(&mut payload) };
            return Err(bridge_message(error));
        }
        let restore_started = Instant::now();
        let status = unsafe {
            gf_finalcut_instance_load_project_payload(
                restored,
                payload.data,
                payload.len,
                &mut error,
            )
        };
        restore_micros.push(restore_started.elapsed().as_micros() as u64);
        unsafe {
            gf_finalcut_instance_free(restored);
            gf_finalcut_owned_bytes_free(&mut payload);
        }
        if status != GFStatus::Ok {
            return Err(format!("payload restore failed: {}", bridge_message(error)));
        }
    }
    let average = |values: &[u64]| -> f64 {
        values.iter().sum::<u64>() as f64 / values.len() as f64 / 1000.0
    };
    Ok(serde_json::json!({
        "schema_version": 1,
        "project": project_path,
        "project_bytes": project.len(),
        "payload_bytes": payload_bytes,
        "payload_to_project_ratio": payload_bytes as f64 / project.len() as f64,
        "iterations": iterations,
        "average_encode_ms": average(&encode_micros),
        "average_restore_ms": average(&restore_micros),
        "host_parameter_round_trip_validated": false,
        "release_blocked_until_host_round_trip": true
    }))
}

fn main() {
    match run() {
        Ok(report) => println!("{}", serde_json::to_string_pretty(&report).unwrap()),
        Err(message) => {
            eprintln!("Final Cut payload capacity probe failed: {message}");
            std::process::exit(2);
        }
    }
}
