use keyphore_lib::lighting::{Air75MainState, merge_main_preserving_side};
use keyphore_lib::protocol::{
    NuPhyRequest, NuPhySessionKey, ProtocolError, build_session_request, validate_response,
    validate_session_response,
};

fn checksum(report: &[u8; 64]) -> u8 {
    report[4..]
        .iter()
        .fold(0, |sum, byte| sum.wrapping_add(*byte))
}

#[test]
fn write_request_encodes_the_verified_64_byte_wire_format() {
    let request = NuPhyRequest::write(0, &[3, 30, 3]).unwrap();
    let report = request.encode(NuPhySessionKey::new(0x5a));
    assert_eq!(report.len(), 64);
    assert_eq!(report[0..2], [0x55, 0xd6]);
    assert_eq!(report[4..8], [0x59, 0x5a, 0x5a, 0x5a]);
    assert_eq!(report[8..11], [0x59, 0x44, 0x59]);
    assert_eq!(report[3], checksum(&report));
}

#[test]
fn request_rejects_payloads_larger_than_56_bytes() {
    assert_eq!(
        NuPhyRequest::write(0, &[0; 57]).unwrap_err(),
        ProtocolError::PayloadTooLarge
    );
}

#[test]
fn temporary_session_response_must_echo_the_challenge_with_one_key() {
    let challenge = std::array::from_fn::<_, 56, _>(|index| index as u8);
    let key = 0x5a;
    let mut response = [0_u8; 64];
    response[0] = 0xaa;
    response[1] = 0xee;
    response[4..8].fill(key);
    for (target, source) in response[8..].iter_mut().zip(challenge) {
        *target = source ^ key;
    }
    response[3] = checksum(&response);
    assert_eq!(
        validate_session_response(&response, &challenge).unwrap(),
        NuPhySessionKey::new(key)
    );
    response[12] ^= 1;
    response[3] = checksum(&response);
    assert_eq!(
        validate_session_response(&response, &challenge).unwrap_err(),
        ProtocolError::SessionMismatch
    );
}

#[test]
fn temporary_session_request_contains_the_plain_challenge() {
    let challenge = std::array::from_fn::<_, 56, _>(|index| index as u8);
    let report = build_session_request(&challenge);
    assert_eq!(report[0..2], [0x55, 0xee]);
    assert_eq!(report[8..], challenge);
    assert_eq!(report[3], checksum(&report));
}

#[test]
fn response_validation_rejects_wrong_address_length_and_session() {
    let key = NuPhySessionKey::new(0x23);
    let request = NuPhyRequest::read(0x1234, 2).unwrap();
    let mut response = [0_u8; 64];
    response[0] = 0xaa;
    response[1] = 0xd5;
    response[4] = 2 ^ key.value();
    response[5] = 0x34 ^ key.value();
    response[6] = 0x12 ^ key.value();
    response[7] = key.value();
    response[8] = 0xab ^ key.value();
    response[9] = 0xcd ^ key.value();
    response[3] = checksum(&response);
    assert_eq!(
        validate_response(&response, &request, key).unwrap(),
        [0xab, 0xcd]
    );
    response[4] = 3 ^ key.value();
    response[3] = checksum(&response);
    assert_eq!(
        validate_response(&response, &request, key).unwrap_err(),
        ProtocolError::ResponseLengthMismatch
    );
    response[4] = 2 ^ key.value();
    response[5] = 0x35 ^ key.value();
    response[3] = checksum(&response);
    assert_eq!(
        validate_response(&response, &request, key).unwrap_err(),
        ProtocolError::ResponseAddressMismatch
    );
    response[5] = 0x34 ^ key.value();
    response[7] = key.value() ^ 1;
    response[3] = checksum(&response);
    assert_eq!(
        validate_response(&response, &request, key).unwrap_err(),
        ProtocolError::SessionMismatch
    );
}

#[test]
fn air75_main_state_replaces_only_the_first_nine_bytes() {
    let existing = [9, 8, 7, 6, 5, 4, 3, 2, 1, 11, 12, 13, 14, 15, 16, 17, 18];
    let merged =
        merge_main_preserving_side(&existing, &Air75MainState::steady(30, 0, 255, 0).unwrap());
    assert_eq!(&merged[..9], &[3, 30, 3, 0, 1, 0, 0, 255, 0]);
    assert_eq!(&merged[9..], &existing[9..]);
}
