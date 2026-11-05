use super::*;

#[test]
fn payload_is_private_form_data_only_and_matches_server_lengths() {
    let value = payload(
        " Scan issue ",
        " Description with enough detail ",
        " me@example.com ",
        Category::Bug,
    )
    .unwrap();
    assert_eq!(value["app"], "Open Pointcloud Studio");
    assert_eq!(value["category"], "bug");
    assert_eq!(
        value["message"],
        "Scan issue\n\nDescription with enough detail"
    );
    assert_eq!(value["email"], "me@example.com");
    assert_eq!(value.as_object().unwrap().len(), 5);
    assert!(payload(
        &"😀".repeat(101),
        "Enough description here",
        "",
        Category::General
    )
    .is_err());
    assert!(payload("Title", "short", "", Category::General).is_err());
    assert!(payload(
        "Title",
        "Enough description here",
        "bad@email",
        Category::General
    )
    .is_err());
    assert_eq!(
        payload("Title", "Enough description here", "", Category::Feature).unwrap()["email"],
        Value::Null
    );
}

#[test]
fn only_an_explicit_private_acknowledgement_is_success() {
    assert!(acknowledgement(
        200,
        r#"{"success":true,"visibility":"private","githubIssueUrl":null,"imageKeys":[]}"#
    )
    .is_ok());
    for response in [
        "<html>error</html>",
        r#"{"success":true}"#,
        r#"{"success":true,"visibility":"public"}"#,
        r#"{"success":true,"visibility":"private","githubIssueUrl":"https://github.com/issue"}"#,
    ] {
        assert_eq!(acknowledgement(200, response), Err(UNAVAILABLE));
    }
    assert_eq!(acknowledgement(409, "{}"), Err(DUPLICATE));
    assert_eq!(acknowledgement(429, "{}"), Err(RATE_LIMIT));
    assert_eq!(acknowledgement(500, "{}"), Err(UNAVAILABLE));
}

#[test]
fn mock_service_checks_the_real_http_contract_without_sending_live_feedback() {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/api/private-feedback", server.server_addr());
    let worker = std::thread::spawn(move || {
        let mut request = server.recv().unwrap();
        assert_eq!(request.url(), "/api/private-feedback");
        assert_eq!(request.method(), &tiny_http::Method::Post);
        let mut body = String::new();
        request.as_reader().read_to_string(&mut body).unwrap();
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["category"], "feature");
        assert!(body.get("images").is_none());
        request
            .respond(tiny_http::Response::from_string(
                r#"{"success":true,"visibility":"private","githubIssueUrl":null}"#,
            ))
            .unwrap();
    });
    assert!(submit_to(
        &endpoint,
        payload(
            "A feature",
            "A useful feature description",
            "",
            Category::Feature
        )
        .unwrap()
    )
    .is_ok());
    worker.join().unwrap();
}

#[test]
fn failure_keeps_the_draft_success_clears_it_and_reopening_keeps_pending_work() {
    let mut studio = Studio::default();
    studio.feedback.title = "Title".into();
    studio.feedback.description = text_editor::Content::with_text("A detailed description");
    studio.feedback.pending = true;
    let _ = studio.feedback_action(Action::Close);
    let _ = studio.feedback_action(Action::Open);
    assert!(studio.feedback.pending);
    let _ = studio.feedback_action(Action::Title("Replacement".into()));
    assert_eq!(studio.feedback.title, "Title");
    let _ = studio.feedback_action(Action::Ready(Err(UNAVAILABLE)));
    assert_eq!(
        studio.feedback.description.text().trim(),
        "A detailed description"
    );
    assert_eq!(studio.feedback.error, Some(UNAVAILABLE));
    studio.feedback.pending = true;
    let _ = studio.feedback_action(Action::Ready(Ok(())));
    assert!(studio.feedback.sent);
    assert!(studio.feedback.title.is_empty());
    assert!(studio.feedback.description.text().trim().is_empty());
}

#[test]
fn feedback_form_renders_for_both_languages_and_all_themes() {
    use crate::i18n::{Language, TestLanguage};
    for language in [Language::English, Language::Table(0)] {
        let _language = TestLanguage::hold(language);
        for theme in crate::ui_theme::UiTheme::ALL {
            let mut studio = Studio::default();
            studio.ui_theme = theme;
            let _ = studio.feedback_action(Action::Open);
            studio.window_size = Some(iced::Size::new(1280.0, 760.0));
            let picture =
                crate::test_render::render_window(&studio, iced::Size::new(1280.0, 760.0));
            if language == Language::English && theme == crate::ui_theme::UiTheme::Light {
                if let Some(directory) = std::env::var_os("OPS_UI_REVIEW_DIR") {
                    let directory = std::path::Path::new(&directory);
                    std::fs::create_dir_all(directory).unwrap();
                    picture.save_png(&directory.join("feedback-form.png"));
                }
            }
            assert!(studio.model_covered());
        }
    }
}
