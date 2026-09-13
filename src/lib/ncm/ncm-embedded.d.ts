declare module "ncm-embedded/browser" {
	export interface EmbeddedApiAnswer {
		status: number;
		body: Record<string, any>;
		cookie: string[];
	}

	export interface EmbeddedExternals {
		fetchImpl: (
			url: string,
			options?: {
				method?: string;
				headers?: Record<string, string>;
				data?: string | ArrayBuffer | Uint8Array;
				followRedirects?: boolean;
			},
		) => Promise<{
			status: number;
			headers: Record<string, string>;
			setCookies: string[];
			body: string;
			bodyIsBase64: boolean;
		}>;
		unblockImpl: {
			matchID: (
				id: string,
				source?: string | null,
			) => Promise<{
				code: number;
				message: string;
				data: { url: string; source: string } | null;
			}>;
		};
	}

	export type EmbeddedRequest = (
		uri: string,
		data: Record<string, any>,
		options: Record<string, any>,
	) => Promise<EmbeddedApiAnswer>;

	export function callEmbeddedApi(
		path: string,
		params: Record<string, any>,
		requestImpl: EmbeddedRequest,
		externals: EmbeddedExternals,
	): Promise<EmbeddedApiAnswer>;

	export function getModuleName(path: string): string;
}
