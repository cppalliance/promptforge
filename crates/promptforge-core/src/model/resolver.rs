//! The semantic `models.need` resolution adapter over the tool picker.

use promptforge_tool_picker::ToolPicker;

use super::{
    ModelCatalog, ModelInvocation, ModelNeedOpts, ModelResolver, ResolvedModel,
    model_from_picker_id, picker_catalog_from,
};
use crate::{Error, Result};

/// Resolver that filters the catalog, then semantically resolves via a picker.
#[derive(Debug)]
pub(crate) struct PickerModelResolver<'a> {
    catalog: &'a ModelCatalog,
    picker: &'a ToolPicker,
}

impl<'a> PickerModelResolver<'a> {
    /// Borrows a catalog and a picker built over that catalog's descriptors.
    #[must_use]
    pub(crate) fn new(catalog: &'a ModelCatalog, picker: &'a ToolPicker) -> Self {
        Self { catalog, picker }
    }
}

impl ModelResolver for PickerModelResolver<'_> {
    fn resolve(&self, description: &str, opts: &ModelNeedOpts) -> Result<ResolvedModel> {
        // Borrowed filtered view (MODEL-017): no full-descriptor clone, and the
        // picker is built directly from these borrowed matches.
        let matches = self.catalog.filtered(opts);
        if matches.is_empty() {
            return Err(Error::ModelAbsent {
                capability: description.to_owned(),
            });
        }
        let picker = self
            .picker
            .rebuild(picker_catalog_from(matches.iter().copied()))
            .map_err(|error| Error::ModelBindQuery {
                capability: description.to_owned(),
                source: crate::error::SharedSource::new(error),
            })?;
        match picker.resolve(description) {
            Ok(promptforge_tool_picker::Outcome::Bind(tool)) => {
                let id = model_from_picker_id(tool.id());
                // The picker was rebuilt from `matches`, so a selected id absent
                // from it is an encoding/consistency fault, not a bind. Fail
                // explicitly instead of fabricating OpenAI + zero-context metadata.
                let descriptor = matches
                    .iter()
                    .copied()
                    .find(|model| *model.id() == id)
                    .ok_or_else(|| Error::ModelBind {
                        capability: description.to_owned(),
                        detail: format!(
                            "picker selected model {}/{} which is absent from the filtered live catalog",
                            id.server(),
                            id.name()
                        ),
                    })?;
                Ok(ResolvedModel {
                    id,
                    invocation: ModelInvocation::from(opts),
                    tool_dialect: descriptor.tool_dialect(),
                    context: descriptor.context(),
                })
            }
            Ok(promptforge_tool_picker::Outcome::Absent) => Err(Error::ModelAbsent {
                capability: description.to_owned(),
            }),
            Ok(promptforge_tool_picker::Outcome::Duplicate(group)) => Err(Error::ModelDuplicate {
                capability: description.to_owned(),
                candidates: group
                    .iter()
                    .map(|tool| model_from_picker_id(tool.id()))
                    .collect(),
            }),
            Ok(promptforge_tool_picker::Outcome::Ambiguous(group)) => Err(Error::ModelAmbiguous {
                capability: description.to_owned(),
                candidates: group
                    .iter()
                    .map(|tool| model_from_picker_id(tool.id()))
                    .collect(),
            }),
            Ok(_) => Err(Error::ModelBind {
                capability: description.to_owned(),
                detail: "the picker reported an unrecognized outcome".to_owned(),
            }),
            Err(error) => Err(Error::ModelBindQuery {
                capability: description.to_owned(),
                source: crate::error::SharedSource::new(error),
            }),
        }
    }
}
