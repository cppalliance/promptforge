# Gateway web search

This crate is the gateway's web search service. It validates each search request, calls the search provider, and cleans and filters the results before returning them. The provider credential stays inside the gateway and leaves only in the call to the provider.
