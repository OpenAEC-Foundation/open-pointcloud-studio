//! The OpenAPI 3.1 document of the REST routes, built from the tool table
//! and from nothing else, so that it can never lag behind the tools.

use serde_json::{json, Map, Value};

use crate::tools;

/// The first sentence of a description, as the summary of its operation.
fn first_sentence(text: &str) -> String {
    let end = text.find(". ").map_or(text.len(), |at| at + 1);
    text[..end].chars().take(200).collect()
}

/// The error statuses every tool route can answer.
const ERROR_STATUSES: [&str; 10] = [
    "400", "401", "403", "404", "405", "413", "422", "500", "503", "504",
];

/// `GET /v1/openapi.json`.
pub fn document() -> Value {
    let mut paths = Map::new();
    for tool in tools::table() {
        let result = if tool.file_result {
            "#/components/schemas/FileResult"
        } else {
            "#/components/schemas/Inspection"
        };
        let mut responses = Map::new();
        responses.insert(
            "200".into(),
            json!({
                "description": "The result of the tool.",
                "content": {"application/json": {"schema": {"$ref": result}}},
            }),
        );
        for status in ERROR_STATUSES {
            responses.insert(
                status.into(),
                json!({"$ref": "#/components/responses/Fout"}),
            );
        }
        paths.insert(
            format!("/v1/tools/{}", tool.name),
            json!({
                "post": {
                    "operationId": tool.name,
                    "summary": first_sentence(tool.description),
                    "description": tool.description,
                    "security": [{"sleutel": []}],
                    "requestBody": {
                        "required": true,
                        "content": {"application/json": {
                            "schema": tool.input_schema(),
                            "example": tool.example(),
                        }},
                    },
                    "responses": responses,
                }
            }),
        );
    }
    let bounds =
        json!({"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3});
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Open Pointcloud Studio API",
            "version": crate::VERSION,
            "description": "Point-cloud files in the request: inspect them, convert them, draw a section through them and mesh them, with the operations of the Open Pointcloud Studio application. Files travel as base64 in the JSON body. The same tools answer as an MCP server at /mcp.",
        },
        "servers": [],
        "paths": paths,
        "components": {
            "securitySchemes": {"sleutel": {"type": "http", "scheme": "bearer"}},
            "schemas": {
                "OutputFile": {
                    "type": "object",
                    "required": ["format", "media_type", "file_name", "bytes", "sha256", "data"],
                    "properties": {
                        "format": {"type": "string"},
                        "media_type": {"type": "string"},
                        "file_name": {"type": "string"},
                        "bytes": {"type": "integer", "minimum": 0},
                        "sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$"},
                        "data": {"type": "string", "contentEncoding": "base64"},
                    },
                },
                "FileResult": {
                    "type": "object",
                    "required": ["output", "report"],
                    "properties": {
                        "output": {"$ref": "#/components/schemas/OutputFile"},
                        "report": {"type": "object"},
                    },
                },
                "Inspection": {
                    "type": "object",
                    "required": ["report"],
                    "properties": {"report": {
                        "type": "object",
                        "required": ["format", "points", "bounds", "attributes", "stations", "station_photos"],
                        "properties": {
                            "format": {"type": "string"},
                            "points": {"type": "integer", "minimum": 0},
                            "bounds": {"type": "object", "properties": {"min": bounds, "max": bounds}},
                            "attributes": {"type": "object", "properties": {
                                "rgb": {"type": "boolean"},
                                "intensity": {"type": "boolean"},
                                "classification": {"type": "boolean"},
                            }},
                            "stations": {"type": "array", "items": {"type": "object", "properties": {
                                "label": {"type": "string"},
                                "position": bounds,
                                "axes": {"type": ["array", "null"], "items": bounds},
                            }}},
                            "station_photos": {"type": "integer", "minimum": 0},
                        },
                    }},
                },
                "Fout": {
                    "type": "object",
                    "required": ["error_code", "melding", "remedie"],
                    "properties": {
                        "error_code": {"type": "string"},
                        "melding": {"type": "string"},
                        "remedie": {"type": ["string", "null"]},
                    },
                },
            },
            "responses": {
                "Fout": {
                    "description": "An error: a code for programs, what went wrong, and what to do about it where something can be done.",
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Fout"}}},
                },
            },
        },
    })
}
