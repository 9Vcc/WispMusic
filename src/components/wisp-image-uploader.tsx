import { Edit24Filled } from "@fluentui/react-icons";
import { useEffect, useRef, useState } from "react";
import Cropper, { type Area } from "react-easy-crop";
import { cn, getCropppedImg } from "@/lib/utils";
import {
	Dialog,
	DialogAction,
	DialogBody,
	DialogCancel,
	DialogContent,
	DialogFooter,
	DialogTitle,
} from "./ui/dialog";
/**
 * createObjectURL 创建的资源不会被 GC 回收，必须显式 revoke：否则每选一次图
 * 就把整张原图与裁剪结果永久留在内存里（组件卸载也不会释放）。
 * 放在模块作用域，避免作为 effect 依赖反复变化。
 */
function releaseBlobUrl(url: string | null) {
	if (url?.startsWith("blob:")) URL.revokeObjectURL(url);
}

export function WispImageUploader({
	src,
	alt,
	onChange,
	className,
	cropShape = "rect",
}: {
	src: string;
	alt: string;
	onChange: (file: File) => void;
	className?: string;
	cropShape?: "rect" | "round";
}) {
	const [previewUrl, setPreviewUrl] = useState(src);
	const [imageToCrop, setImageToCrop] = useState<string | null>(null);
	const fileInputRef = useRef<HTMLInputElement>(null);
	// 本组件自己创建的 blob URL，替换与卸载时都要回收
	const imageToCropUrlRef = useRef<string | null>(null);
	const previewBlobUrlRef = useRef<string | null>(null);

	useEffect(() => {
		return () => {
			releaseBlobUrl(imageToCropUrlRef.current);
			releaseBlobUrl(previewBlobUrlRef.current);
			imageToCropUrlRef.current = null;
			previewBlobUrlRef.current = null;
		};
	}, []);

	const closeCropper = () => {
		releaseBlobUrl(imageToCropUrlRef.current);
		imageToCropUrlRef.current = null;
		setImageToCrop(null);
	};

	const handleFileChange = (e: React.ChangeEvent<HTMLInputElement>) => {
		const file = e.target.files?.[0];
		if (file) {
			releaseBlobUrl(imageToCropUrlRef.current);
			const url = URL.createObjectURL(file);
			imageToCropUrlRef.current = url;
			setImageToCrop(url);
		}
		e.target.value = "";
	};
	return (
		<>
			{" "}
			<div
				className={cn(
					"size-48 relative rounded-lg overflow-hidden shrink-0 drop-shadow-xl group",
					className,
				)}
				onClick={() => fileInputRef.current?.click()}
			>
				{" "}
				<img
					className="size-full group-hover:brightness-50 transition-all duration-300 object-cover"
					src={previewUrl}
					alt={alt}
				/>{" "}
				<Edit24Filled className="size-8 absolute right-1/2 bottom-1/2 translate-1/2 text-white opacity-0 group-hover:opacity-100 transition-opacity duration-300" />{" "}
				<input
					type="file"
					ref={fileInputRef}
					accept="image/*"
					className="hidden"
					onChange={handleFileChange}
				/>{" "}
			</div>{" "}
			<WispCropperDialog
				image={imageToCrop}
				cropShape={cropShape}
				onClose={closeCropper}
				onConfirm={(url, file) => {
					// 裁剪结果就地顶替预览图：旧的预览 blob 与裁剪源同时回收
					releaseBlobUrl(imageToCropUrlRef.current);
					imageToCropUrlRef.current = null;
					releaseBlobUrl(previewBlobUrlRef.current);
					previewBlobUrlRef.current = url.startsWith("blob:") ? url : null;
					setPreviewUrl(url);
					setImageToCrop(null);
					onChange(file);
				}}
			/>{" "}
		</>
	);
}
function WispCropperDialog({
	image,
	cropShape = "rect",
	onClose,
	onConfirm,
}: {
	image: string | null;
	cropShape?: "rect" | "round";
	onClose: () => void;
	onConfirm: (url: string, file: File) => void;
}) {
	const [crop, setCrop] = useState({ x: 0, y: 0 });
	const [zoom, setZoom] = useState(1);
	const [croppedAreaPixels, setCroppedAreaPixels] = useState<Area | null>(null);
	const onCropComplete = (_: Area, croppedAreaPixels: Area) => {
		setCroppedAreaPixels(croppedAreaPixels);
	};
	const handleConfirm = async () => {
		if (!image) return;
		try {
			const croppedBlob = await getCropppedImg(image, croppedAreaPixels!);
			const croppedImgUrl = URL.createObjectURL(croppedBlob as Blob);
			const croppedFile = new File(
				[croppedBlob as Blob],
				"playlist_cover_cropped.jpg",
				{ type: "image/jpeg" },
			);
			onConfirm(croppedImgUrl, croppedFile);
		} catch (error) {
			console.error(error);
		}
	};
	return (
		<Dialog
			open={!!image}
			onOpenChange={(val) => {
				if (!val) onClose();
			}}
		>
			{" "}
			<DialogContent className="sm:max-w-xl">
				{" "}
				<DialogTitle className="sr-only">裁剪图片</DialogTitle>{" "}
				<DialogBody>
					{" "}
					<div className="flex flex-col gap-8 p-4 pt-6">
						{" "}
						<div className="relative w-full h-100 bg-card/5 rounded-lg overflow-hidden">
							{" "}
							{image && (
								<Cropper
									image={image}
									crop={crop}
									zoom={zoom}
									aspect={1}
									cropShape={cropShape}
									onCropChange={setCrop}
									onZoomChange={setZoom}
									onCropComplete={onCropComplete}
									objectFit="contain"
								/>
							)}{" "}
						</div>{" "}
						<p className="text-center text-sm text-foreground/50">
							{" "}
							拖拽、缩放图片以进行裁剪{" "}
						</p>{" "}
					</div>{" "}
				</DialogBody>{" "}
				<DialogFooter>
					{" "}
					<DialogCancel onClick={onClose}>取消</DialogCancel>{" "}
					<DialogAction onClick={handleConfirm}>确定</DialogAction>{" "}
				</DialogFooter>{" "}
			</DialogContent>{" "}
		</Dialog>
	);
}
