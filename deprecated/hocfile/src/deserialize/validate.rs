use super::*;

impl Hocfile {
    pub(super) fn validate_conflicts(&self, errors: &mut Vec<HocfileError>) -> bool {
        let err_len = errors.len();

        helper(errors, CMD, None, self.commands.iter().map(|cmd| &cmd.name));
        helper(
            errors,
            OPS,
            None,
            self.optional_sets.iter().map(|ops| &ops.name),
        );
        helper(
            errors,
            SCR,
            None,
            self.script.predefined.iter().map(|scr| &scr.name),
        );

        for command in &self.commands {
            helper(
                errors,
                ARG,
                Some((CMD, &command.name)),
                command.arguments.iter().map(|arg| &arg.name),
            );
            helper_optionals(errors, CMD, &command.name, &command.optionals);
        }

        for optional_set in &self.optional_sets {
            helper_optionals(errors, OPS, &optional_set.name, &optional_set.optionals);
        }

        fn helper<'a>(
            errors: &mut Vec<HocfileError>,
            resource_type: &'static str,
            parent: Option<(&'static str, &'a str)>,
            collection: impl IntoIterator<Item = &'a ResourceRef>,
        ) {
            let names = collection
                .into_iter()
                .map(|resource_ref| resource_ref.deref())
                .fold(HashMap::new(), |mut map, name| {
                    map.entry(name).and_modify(|count| *count += 1).or_insert(1);
                    map
                });

            let dup_names = names.iter().filter(|(_, count)| **count > 1);
            if dup_names.clone().count() > 0 {
                errors.extend(
                    dup_names.map(|(name, _)| HocfileError::MultipleDefinitions {
                        resource_type,
                        resource_name: name.to_string(),
                        parent: parent.map(|(t, n)| (t, n.to_string())),
                    }),
                )
            }
        }

        fn helper_optionals<'a, I>(
            errors: &mut Vec<HocfileError>,
            resource_type: &'static str,
            resource_name: &'a str,
            optionals: I,
        ) where
            I: IntoIterator<Item = &'a Optional>,
            I::IntoIter: Clone,
        {
            let optionals_iter = optionals.into_iter();
            let concrete_optionals_iter = optionals_iter
                .clone()
                .filter_map(Optional::as_concrete_optional)
                .map(|concrete_optional| &concrete_optional.name);
            let optional_set_refs_iter = optionals_iter.filter_map(Optional::as_optional_set_ref);

            helper(
                errors,
                OPL,
                Some((resource_type, resource_name)),
                concrete_optionals_iter,
            );
            helper(
                errors,
                OPR,
                Some((resource_type, resource_name)),
                optional_set_refs_iter,
            );
        }

        errors.len() > err_len
    }

    pub(super) fn validate_references(&self, errors: &mut Vec<HocfileError>) -> bool {
        let err_len = errors.len();

        for command in &self.commands {
            helper(
                errors,
                SCR,
                CMD,
                &command.name,
                self.script.predefined.iter().map(|scr| &scr.name),
                command
                    .procedure
                    .iter()
                    .filter_map(|step| step.step_type.as_script_ref()),
            );
            helper(
                errors,
                OPR,
                CMD,
                &command.name,
                self.optional_sets.iter().map(|ops| &ops.name),
                command
                    .optionals
                    .iter()
                    .filter_map(|opt| opt.as_optional_set_ref()),
            );
        }

        for optional_set in &self.optional_sets {
            helper(
                errors,
                OPR,
                OPS,
                &optional_set.name,
                self.optional_sets.iter().map(|ops| &ops.name),
                optional_set
                    .optionals
                    .iter()
                    .filter_map(|opt| opt.as_optional_set_ref()),
            );
        }

        fn helper<'a>(
            errors: &mut Vec<HocfileError>,
            resource_type: &'static str,
            parent_type: &'static str,
            parent_name: &'a str,
            resources: impl IntoIterator<Item = &'a ResourceRef>,
            collection: impl IntoIterator<Item = &'a ResourceRef>,
        ) {
            let mut names: HashSet<_> = collection
                .into_iter()
                .map(|resource_ref| resource_ref.deref())
                .collect();
            for resource_ref in resources.into_iter() {
                names.remove(&resource_ref.deref());
            }

            if names.len() > 0 {
                errors.extend(names.into_iter().map(|name| HocfileError::MissingResource {
                    resource_type,
                    resource_name: name.to_string(),
                    parent_type,
                    parent_name: parent_name.to_string(),
                }))
            }
        }

        errors.len() > err_len
    }

    pub(super) fn validate_cyclic_dependencies(&self, errors: &mut Vec<HocfileError>) -> bool {
        let (nodes, edges) = self.get_optional_set_nodes_and_edges();

        if let Err(err) = Tree::new(nodes, edges) {
            errors.push(HocfileError::CyclicReferences {
                resource_type: OPS,
                references: err.iter().map(|ops| ops.name.0.clone()).collect(),
            });
            true
        } else {
            false
        }
    }

    pub(super) fn get_optional_set_nodes_and_edges(
        &self,
    ) -> (IndexSet<&OptionalSet>, IndexSet<Edge>) {
        let nodes: IndexSet<_> = self.optional_sets.iter().collect();

        let find_optional_set =
            |name: &ResourceRef| nodes.iter().position(|node| node.name == *name).unwrap();

        let edges: IndexSet<_> = self
            .optional_sets
            .iter()
            .flat_map(|optional_set| {
                let to = find_optional_set(&optional_set.name);

                optional_set
                    .optionals
                    .iter()
                    .filter_map(move |optional| match optional {
                        Optional::Set { from_optional_set } => Some(Edge {
                            from: find_optional_set(from_optional_set),
                            to,
                        }),
                        _ => None,
                    })
            })
            .collect();

        (nodes, edges)
    }
}
